#include "pch_App.h"
#include "Pilot.h"

#include "../Portal.h"

namespace Modules {
	namespace PerPortal {
		//----------
		Pilot::Pilot(Portal * portal)
			: portal(portal)
		{
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
			};
		}

		//----------
		string
			Pilot::getTypeName() const
		{
			return "Pilot";
		}

		//----------
		string
			Pilot::getGlyph() const
		{
			return u8"\uf655";
		}

		//----------
		void
			Pilot::init()
		{

		}

		//----------
		void
			Pilot::update()
		{
			// Calculate polar angles
			if (this->parameters.position.enabled) {
				const auto x = this->parameters.position.x;
				const auto y = this->parameters.position.y;
				auto r = glm::length(glm::vec2(x, y));
				auto theta = atan2(y, x);

				// clamp max r value
				if (r > 1.0f) {
					r = 1.0f;
					this->parameters.position.x.set(cos(theta) * r);
					this->parameters.position.y.set(sin(theta) * r);
				}

				this->parameters.polar.r.set(r);
				this->parameters.polar.theta.set(theta);
			}

			// Calculate prism step positions
			if (this->parameters.polar.enabled) {
				auto thetaNorm = this->parameters.polar.theta / TWO_PI;
				auto r = this->parameters.polar.r;
				this->parameters.axes.a = thetaNorm - r * 0.25;
				this->parameters.axes.b = 0.5 - (thetaNorm + r * 0.25);
			}

			// Check if needs push
			{
				auto now = chrono::system_clock::now();
				if (now - this->cachedSentValues.lastUpdate > this->cachedSentValues.updatePeriod
					|| this->parameters.axes.a != this->cachedSentValues.a
					|| this->parameters.axes.b != this->cachedSentValues.b)
				{
					this->pushValues();
				}
			}
		}

		//----------
		void
			Pilot::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;

			inspector->add(this->getPanel());
			inspector->addButton("Reset position", [this]() {
				this->parameters.position.x = 0;
				this->parameters.position.y = 0;
				}, 'r');
			inspector->addParameterGroup(this->parameters);
		}

		//----------
		ofxCvGui::PanelPtr
			Pilot::getPanel()
		{
			auto horizontalStrip = ofxCvGui::Panels::Groups::makeStrip(ofxCvGui::Panels::Groups::Strip::Direction::Horizontal);
			{
				horizontalStrip->setCellSizes({ 300, 100 });
				horizontalStrip->setWidth(100.0f);
				horizontalStrip->setHeight(400.0f);
			}
			{
				auto power = 0.2f;

				auto panel = ofxCvGui::Panels::makeBlank();
				panel->onDraw += [this, power](ofxCvGui::DrawArguments& args)
				{
					auto panelCenter = args.localBounds.getCenter();
					auto panelSize = min(args.localBounds.width, args.localBounds.height);

					ofPushMatrix();
					{
						ofTranslate(panelCenter);
						ofScale(panelSize / 2.0f, panelSize / 2.0f);

						ofPushStyle();
						{
							ofNoFill();
							ofSetColor(100);
							ofSetCircleResolution(100);

							// Draw faint lines
							for (float r = 0.1; r <= 1.0f; r += 0.1f) {
								ofDrawCircle(0, 0, pow(r, power));
							}

							// Draw bolder lines
							ofSetColor(200);
							ofDrawCircle(0, 0, pow(0.5, power));
							ofDrawCircle(0, 0, 1.0);
							ofDrawLine(-1, 0, 1, 0);
							ofDrawLine(0, -1, 0, 1);
						}
						ofPopStyle();
					}
					ofPopMatrix();

					{
						auto x = this->parameters.position.x;
						auto y = this->parameters.position.y;
						auto r = glm::length(glm::vec2(x, y));
						auto theta = atan2(y, x);
						auto r_view = pow(r, power);

						ofDrawCircle(
							ofMap(r_view * cos(theta)
								, -1
								, 1
								, panelCenter.x - panelSize / 2
								, panelCenter.x + panelSize / 2)
							, ofMap(r_view * sin(theta)
								, -1
								, 1
								, panelCenter.y - panelSize / 2
								, panelCenter.y + panelSize / 2)
							, 10.0f
						);
					}
				};
				panel->onMouse += [this, panel, power](ofxCvGui::MouseArguments& args)
				{
					args.takeMousePress(panel);

					if (args.isDragging(panel)) {
						// Multiply movements by current r value 
						auto x = this->parameters.position.x.get();
						auto y = this->parameters.position.x.get();
						auto r = glm::length(glm::vec2(x, y));
						auto r_factor = pow(ofClamp(r, 0.0001, 1), power); // allow for movements when at 0,0

						auto movement = r_factor * args.movement / glm::vec2(panel->getWidth(), panel->getHeight());
						this->parameters.position.x += movement.x;
						this->parameters.position.y += movement.y;
						this->parameters.position.x = ofClamp(this->parameters.position.x, -1, 1);
						this->parameters.position.y = ofClamp(this->parameters.position.y, -1, 1);
					}
				};

				{
					auto slider = make_shared<ofxCvGui::Widgets::Slider>(this->parameters.axes.offset);
					slider->setBounds({ 0, 0, 200, 40 });
					panel->addChild(slider);
				}


				horizontalStrip->add(panel);
			}

			{
				auto verticalStrip = ofxCvGui::Panels::Groups::makeStrip(ofxCvGui::Panels::Groups::Strip::Direction::Vertical);
				{
					auto makeAxisControlPanel = [this](ofParameter<float>& axis) {
						auto horizontalStrip = ofxCvGui::Panels::Groups::makeStrip(ofxCvGui::Panels::Groups::Strip::Direction::Horizontal);
						horizontalStrip->setCellSizes({ 100, -1 });
						{
							auto buttonStrip = ofxCvGui::Panels::makeWidgets();
							auto go = [&axis, this](float position) {
								this->parameters.polar.enabled.set(false);
								axis.set(position);
							};
							map<float, string> positions;
							{
								positions[0] = "Left";
								positions[0.25] = "Up";
								positions[0.5] = "Right";
								positions[0.75] = "Down";
							}
							map<string, string> icons;
							{
								icons["Left"] = u8"\uf137";
								icons["Up"] = u8"\uf139";
								icons["Right"] = u8"\uf138";
								icons["Down"] = u8"\uf13a";
							}
							for (auto it : positions) {
								auto value = it.first;
								buttonStrip->addButton(ofToString(value), [go, value]() {
									go(value);
									})->setDrawGlyph(icons[it.second]);
							}

							horizontalStrip->add(buttonStrip);
						}
						{
							auto panel = ofxCvGui::Panels::makeBlank();
							panel->setWidth(100.0f);
							panel->setHeight(100.0f);
							panel->onDraw += [this, &axis](ofxCvGui::DrawArguments& args)
							{
								auto panelCenter = args.localBounds.getCenter();
								auto panelSize = min(args.localBounds.width, args.localBounds.height);

								ofPushMatrix();
								{
									ofTranslate(panelCenter);
									ofScale(panelSize / 2.0f, panelSize / 2.0f);

									ofPushStyle();
									{
										ofNoFill();
										ofSetColor(150);
										ofSetCircleResolution(32);
										ofDrawCircle(0, 0, 1.0f);
										ofDrawLine(-1, 0, 1, 0);
										ofDrawLine(0, -1, 0, 1);
									}
									ofPopStyle();
								}
								ofPopMatrix();

								{
									auto theta = ofMap(axis.get()
										, 0
										, 1
										, 0
										, TWO_PI) + PI;
									auto r = 0.75f;
									auto x = r * cos(theta);
									auto y = r * sin(theta);

									auto panel_x = ofMap(x, -1, 1
										, panelCenter.x - panelSize / 2
										, panelCenter.x + panelSize / 2);
									auto panel_y = ofMap(y, -1, 1
										, panelCenter.y - panelSize / 2
										, panelCenter.y + panelSize / 2);
									ofPushStyle();
									{
										if (this->parameters.polar.enabled) {
											ofNoFill();
										}
										ofDrawCircle(panel_x, panel_y, 10.0f);
									}
									ofPopStyle();
									ofDrawLine(panelCenter.x, panelCenter.y, panel_x, panel_y);

									{
										ofRectangle textBounds(panelCenter - glm::vec2(20, 20), 40, 40);
										auto text = axis.getName() + "\n" + ofToString(axis.get(), 3);
										ofxCvGui::Utils::drawText(text, textBounds);
									}
								}
							};
							panel->onMouse += [this, panel, &axis](ofxCvGui::MouseArguments& args)
							{
								args.takeMousePress(panel);

								if (args.isDragging(panel)) {
									this->parameters.polar.enabled.set(false);
									auto movement = args.movement / glm::vec2(panel->getWidth(), panel->getHeight());
									axis += movement.x / 10.0f;
								}

								if (args.isDoubleClicked(panel)) {
									auto response = ofSystemTextBoxDialog("Value for " + axis.getName());
									if (!response.empty()) {
										axis.set(ofToFloat(response));
										this->parameters.polar.enabled = false;
									}
								}
							};
							horizontalStrip->add(panel);
						}

						return horizontalStrip;
					};

					verticalStrip->add(makeAxisControlPanel(this->parameters.axes.a));
					verticalStrip->add(makeAxisControlPanel(this->parameters.axes.b));
				}
				horizontalStrip->add(verticalStrip);
			}

			return horizontalStrip;
		}

		//----------
		void
			Pilot::pushValues()
		{
			{
				auto norm = this->parameters.axes.a.get() + this->parameters.axes.offset.get();
				auto steps = (int32_t)ofMap(norm
					, 0
					, 1
					, 0
					, this->parameters.axes.microstepsPerPrismRotation.get());
				this->portal->getAxis(0)->getMotionControl()->move(steps);
			}

			{
				auto norm = this->parameters.axes.b.get() - this->parameters.axes.offset.get();
				auto steps = (int32_t)ofMap(norm
					, 0
					, 1
					, 0
					, -this->parameters.axes.microstepsPerPrismRotation.get());
				this->portal->getAxis(1)->getMotionControl()->move(steps);
			}

			this->cachedSentValues.a = this->parameters.axes.a;
			this->cachedSentValues.b = this->parameters.axes.b;
			this->cachedSentValues.lastUpdate = chrono::system_clock::now();
		}
	}
}
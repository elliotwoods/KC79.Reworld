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
			// Calculate other values from the leading control
			switch (this->parameters.leadingControl.get().get()) {
			case LeadingControl::Position:
			{
				auto position = this->getPosition();
				auto polar = this->positionToPolar(position);

				// clamp max r value
				if (polar[0] > 1.0f) {
					polar[0] = 1.0f;
					position = this->polarToPosition({ polar[0], polar[1] });
					this->setPosition(position);
				}

				{
					this->parameters.polar.r.set(polar[0]);
					this->parameters.polar.theta.set(polar[1]);
				}

				auto axes = this->polarToAxes(polar);
				{
					this->parameters.axes.a.set(axes[0]);
					this->parameters.axes.b.set(axes[1]);
				}
				break;
			}
			case LeadingControl::Polar:
			{
				auto polar = this->getPolar();
				auto position = this->polarToPosition(polar);
				{
					this->parameters.position.x.set(position[0]);
					this->parameters.position.y.set(position[1]);
				}
				auto axes = this->polarToAxes(polar);
				{
					this->parameters.axes.a.set(axes[0]);
					this->parameters.axes.b.set(axes[1]);
				}
				break;
			}
			case LeadingControl::Axes:
			{
				auto axes = this->getAxes();
				auto polar = this->axesToPolar(axes);
				{
					this->parameters.polar.r.set(polar[0]);
					this->parameters.polar.theta.set(polar[1]);
				}
				auto position = this->polarToPosition(polar);
				{
					this->parameters.position.x.set(position[0]);
					this->parameters.position.y.set(position[1]);
				}
				break;
			}
			default:
				break;
			}

			// Check if needs push
			{
				bool needsSend = false;
				{
					if (this->parameters.axes.sendPeriodically) {
						auto now = chrono::system_clock::now();
						if (now - this->cachedSentValues.lastUpdateRequest > this->cachedSentValues.updatePeriod) {
							needsSend = true;
						}
					}
				}
				if (needsSend) {
					this->pushValues();
				}
				else {
					if (this->parameters.axes.a != this->cachedSentValues.a) {
						this->pushA();
					}
					if (this->parameters.axes.b != this->cachedSentValues.b) {
						this->pushB();
					}
				}
			}

			// Update live axis values
			{
				this->liveAxisValues.x = this->stepsToAxis(
					this->portal->getAxis(0)->getMotionControl()->getCurrentPosition()
				);
				this->liveAxisValues.y = this->stepsToAxis(
					this->portal->getAxis(1)->getMotionControl()->getCurrentPosition()
				);
				this->liveAxisTargetValues.x = this->stepsToAxis(
					this->portal->getAxis(0)->getMotionControl()->getTargetPosition()
				);
				this->liveAxisTargetValues.y = this->stepsToAxis(
					this->portal->getAxis(1)->getMotionControl()->getTargetPosition()
				);
			}
		}

		//----------
		void
			Pilot::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;

			inspector->add(this->getPanel());
			inspector->addButton("Reset position", [this]() {
				this->setPosition({ 0.0f, 0.0f });
				}, 'r');
			inspector->addButton("Push axis values", [this]() {
				this->pushValues();
				}, ' ');
			inspector->addButton("Poll", [this]() {
				this->portal->poll();
				}, 'p');
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

				// Create the main position panel
				auto panel = ofxCvGui::Panels::makeBlank();
				auto panelWeak = weak_ptr<ofxCvGui::Panels::Base>(panel);
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

					auto polarToView = [this, panelCenter, panelSize, power](const glm::vec2& polar) {
						const auto& r = polar[0];
						const auto& theta = polar[1];
						auto r_view = pow(r, power);

						return glm::vec2{
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
						};
					};

					// Draw current position (presumes known)
					{
						auto currentPolar = this->axesToPolar(this->liveAxisValues);
						auto currentPositionInView = polarToView(currentPolar);
						ofPushMatrix();
						ofPushStyle();
						{
							ofTranslate(currentPositionInView);
							ofSetColor(200);
							ofDrawLine(-10, 0, 10, 0);
							ofDrawLine(0, -10, 0, 10);
						}
						ofPopStyle();
						ofPopMatrix();
					}

					// Draw cursor
					ofPushStyle();
					{
						if (this->parameters.leadingControl.get() == LeadingControl::Position) {
							ofFill();
						}
						else {
							ofNoFill();
						}
						ofDrawCircle(polarToView(this->getPolar()), 10.0f);
					}
					ofPopStyle();

				};
				panel->onMouse += [this, panelWeak, power](ofxCvGui::MouseArguments& args)
				{
					auto panel = panelWeak.lock();

					args.takeMousePress(panel);

					if (args.isDragging(panel)) {
						// Multiply movements by current r value 
						auto x = this->parameters.position.x.get();
						auto y = this->parameters.position.x.get();
						auto r = glm::length(glm::vec2(x, y));
						auto r_factor = pow(ofClamp(r, 0.0001, 1), power); // allow for movements when at 0,0

						auto movement = r_factor * args.movement / glm::vec2(panel->getWidth(), panel->getHeight());
						auto position = this->getPosition();
						position += movement;
						position.x = ofClamp(position.x, -1, 1);
						position.y = ofClamp(position.y, -1, 1);
						this->setPosition(position);
					}

					if (args.isDoubleClicked(panel)) {
						auto panelSize = min(panel->getWidth(), panel->getHeight());
						auto panelCenter = glm::vec2(panel->getWidth() / 2.0f, panel->getHeight() / 2.0f);
						auto xyPanel = glm::vec2({
							ofMap(args.local.x
								, panelCenter.x - panelSize / 2
								, panelCenter.x + panelSize / 2
								, -1
								, 1)
							, ofMap(args.local.y
								, panelCenter.y - panelSize / 2
								, panelCenter.y + panelSize / 2
								, -1
								, 1)
							});

						auto r_view = glm::length(xyPanel);
						auto theta = atan2(xyPanel[1], xyPanel[0]);
						auto r = pow(r_view, 1.0f / power);
						auto x = r * cos(theta);
						auto y = r * sin(theta);
						this->setPosition({ x, y });
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
					auto makeAxisControlPanel = [this](ofParameter<float>& axis, int axisIndex) {
						auto horizontalStrip = ofxCvGui::Panels::Groups::makeStrip(ofxCvGui::Panels::Groups::Strip::Direction::Horizontal);
						horizontalStrip->setCellSizes({ -1, 80});
						{
							auto panel = ofxCvGui::Panels::makeBlank();
							panel->setWidth(100.0f);
							panel->setHeight(100.0f);
							panel->onDraw += [this, &axis, axisIndex](ofxCvGui::DrawArguments& args)
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
										ofDrawLine(-1, 0, -0.9, 0);
										ofDrawLine(1, 0, 0.9, 0);
										ofDrawLine(0, -1, 0, -0.9);
										ofDrawLine(0, 1, 0, 0.9);
									}
									ofPopStyle();
								}
								ofPopMatrix();

								auto axisValueToPanelPosition = [&](float axisValue, float r) {
									auto theta = ofMap(axisValue
										, 0
										, 1
										, 0
										, TWO_PI) + PI;
									auto x = r * cos(theta);
									auto y = r * sin(theta);

									auto panel_x = ofMap(x, -1, 1
										, panelCenter.x - panelSize / 2
										, panelCenter.x + panelSize / 2);
									auto panel_y = ofMap(y, -1, 1
										, panelCenter.y - panelSize / 2
										, panelCenter.y + panelSize / 2);
									return glm::vec2{
										panel_x
										, panel_y
									};
								};

								// Draw current value
								{
									auto drawPosition = axisValueToPanelPosition(this->liveAxisValues[axisIndex], 0.5f);
									ofPushStyle();
									{
										ofSetColor(200);
										ofDrawLine(panelCenter, { drawPosition.x, drawPosition.y });
										ofDrawCircle({ drawPosition.x, drawPosition.y }, 5.0f);
									}
									ofPopStyle();
								}

								// Draw current target value
								{
									auto drawPosition = axisValueToPanelPosition(this->liveAxisTargetValues[axisIndex], 0.5f);
									ofPushStyle();
									{
										ofNoFill();
										ofSetColor(200);
										ofDrawCircle({ drawPosition.x, drawPosition.y }, 8.0f);
									}
									ofPopStyle();
								}

								// Draw line and circle
								{

									auto drawPosition = axisValueToPanelPosition(axis.get(), 0.75f);

									ofPushStyle();
									{
										if (this->parameters.leadingControl.get() == LeadingControl::Axes) {
											ofFill();
										}
										else {
											ofNoFill();
										}
										ofDrawCircle(drawPosition, 10.0f);
									}
									ofPopStyle();
									ofDrawLine(panelCenter, drawPosition);

									{
										ofRectangle textBounds(panelCenter - glm::vec2(20, 20), 40, 40);
										ofxCvGui::Utils::drawText(axis.getName(), textBounds);
										
										ofxCvGui::Utils::drawText(ofToString(axis.get(), 3), 20, 20);
									}
								}
							};
							panel->onMouse += [this, panel, &axis](ofxCvGui::MouseArguments& args)
							{
								args.takeMousePress(panel);

								if (args.isDragging(panel)) {
									auto movement = args.movement / glm::vec2(panel->getWidth(), panel->getHeight());
									axis.set(axis.get() + movement.x / 10.0f);
									this->parameters.leadingControl.set(LeadingControl::Axes);
								}

								if (args.isDoubleClicked(panel)) {
									auto response = ofSystemTextBoxDialog("Value for " + axis.getName());
									if (!response.empty()) {
										axis.set(ofToFloat(response));
										this->setAxes(this->getAxes()); // invoke set events
									}
								}
							};
							horizontalStrip->add(panel);
						}

						{
							auto buttonStrip = ofxCvGui::Panels::makeWidgets();
							auto go = [&axis, this](float position) {
								axis.set(position);
								this->setAxes(this->getAxes()); // invoke events
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

						return horizontalStrip;
					};

					verticalStrip->add(makeAxisControlPanel(this->parameters.axes.a, 0));
					verticalStrip->add(makeAxisControlPanel(this->parameters.axes.b, 1));
				}
				horizontalStrip->add(verticalStrip);
			}

			return horizontalStrip;
		}

		//----------
		void
			Pilot::pushA()
		{
			auto norm = this->parameters.axes.a.get() + this->parameters.axes.offset.get();
			auto steps = this->axisToSteps(norm);
			this->portal->getAxis(0)->getMotionControl()->move(steps);

			this->cachedSentValues.a = this->parameters.axes.a;
		}

		//----------
		void
			Pilot::pushB()
		{
			auto norm = this->parameters.axes.b.get() - this->parameters.axes.offset.get();
			auto steps = this->axisToSteps(norm);
			this->portal->getAxis(1)->getMotionControl()->move(steps);

			this->cachedSentValues.b = this->parameters.axes.b;
		}

		//----------
		void
			Pilot::pushValues()
		{
			this->pushA();

			ofSleepMillis(100);

			this->pushB();

			this->cachedSentValues.lastUpdateRequest = chrono::system_clock::now();
		}

		//----------
		void
			Pilot::seeThrough()
		{
			this->parameters.axes.a.set(0.0f);
			this->parameters.axes.b.set(1.0f);
			this->parameters.leadingControl.set(LeadingControl::Axes);
		}

		//----------
		const glm::vec2
			Pilot::getPosition() const
		{
			return {
				this->parameters.position.x.get()
				, this->parameters.position.y.get()
			};
		}

		//----------
		const glm::vec2
			Pilot::getPolar() const
		{
			return {
				this->parameters.polar.r.get()
				, this->parameters.polar.theta.get()
			};
		}

		//----------
		const glm::vec2
			Pilot::getAxes() const
		{
			return {
				this->parameters.axes.a.get()
				, this->parameters.axes.b.get()
			};
		}

		//----------
		void
			Pilot::setPosition(const glm::vec2& position)
		{
			this->parameters.position.x.set(position.x);
			this->parameters.position.y.set(position.y);
			this->parameters.leadingControl.set(LeadingControl::Position);
		}

		//----------
		void
			Pilot::setPolar(const glm::vec2& polar)
		{
			this->parameters.polar.r.set(polar[0]);
			this->parameters.polar.theta.set(polar[1]);
			this->parameters.leadingControl.set(LeadingControl::Polar);
		}

		//----------
		void
			Pilot::setAxes(const glm::vec2& axes)
		{
			this->parameters.axes.a.set(axes[0]);
			this->parameters.axes.b.set(axes[1]);
			this->parameters.leadingControl.set(LeadingControl::Axes);
		}

		//----------
		glm::vec2
			Pilot::positionToPolar(const glm::vec2& position) const
		{
			auto r = glm::length(position);
			auto theta = atan2(position.y, position.x);

			return { r, theta };
		}

		//----------
		glm::vec2
			Pilot::polarToPosition(const glm::vec2& polar) const
		{
			const auto& r = polar[0];
			const auto& theta = polar[1];

			return {
				r * cos(theta)
				, r * sin(theta)
			};
		}

		//----------
		glm::vec2
			Pilot::polarToAxes(const glm::vec2& polar) const
		{
			const auto & r = polar[0];
			const auto & theta = polar[1];

			const auto thetaNorm = theta / TWO_PI;

			// our special sauce for our lenses
			return {
				thetaNorm - r * 0.25
				, 0.5 - (thetaNorm + r * 0.25)
			};
		}

		//----------
		// https://www.wolframalpha.com/input?i=systems+of+equations+calculator&assumption=%7B%22F%22%2C+%22SolveSystemOf2EquationsCalculator%22%2C+%22equation1%22%7D+-%3E%22a+%3D+y+-+x%2F4%22&assumption=%22FSelect%22+-%3E+%7B%7B%22SolveSystemOf2EquationsCalculator%22%7D%7D&assumption=%7B%22F%22%2C+%22SolveSystemOf2EquationsCalculator%22%2C+%22equation2%22%7D+-%3E%22b+%3D+1%2F2+-+%28y+%2B+a%2F4%29%22
		glm::vec2
			Pilot::axesToPolar(const glm::vec2& axes) const
		{
			const auto& a = axes[0];
			const auto& b = axes[1];

			auto r = -5 * a - 4 * b + 2;
			auto thetaNorm = (-a - 4*b + 2) / 4;

			auto theta = thetaNorm * TWO_PI;

			return {
				r
				, theta
			};
		}

		//----------
		Steps
			Pilot::axisToSteps(float axisValue) const
		{
			return ofMap(axisValue
				, 0
				, 1
				, 0
				, -this->parameters.axes.microstepsPerPrismRotation.get());
		}

		//----------
		float
			Pilot::stepsToAxis(Steps stepsValue) const
		{
			return ofMap(stepsValue
				, 0
				, -this->parameters.axes.microstepsPerPrismRotation.get()
				, 0
				, 1);
		}
	}
}
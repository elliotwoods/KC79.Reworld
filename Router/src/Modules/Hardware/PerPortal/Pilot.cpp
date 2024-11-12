#include "pch_App.h"
#include "Pilot.h"

#include "../Portal.h"

using namespace msgpack11;

namespace Modules {
	namespace PerPortal {
		//----------
		Pilot::Pilot(Portal* portal)
			: portal(portal)
		{
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->portal->populateInspectorPanelHeader(args);
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

				// position to polar
				auto polar = this->positionToPolar(position);
				{
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
				}

				// polar to axes
				{
					auto axes = this->polarToAxes(polar);

					// cyclical
					if (this->parameters.axes.cyclic) {
						axes = this->findClosestAxesCycle(axes);
					}

					{
						this->parameters.axes.a.set(axes[0]);
						this->parameters.axes.b.set(axes[1]);
					}
				}
				break;
			}
			case LeadingControl::Polar:
			{
				auto polar = this->getPolar();

				// polar to position
				{
					auto position = this->polarToPosition(polar);
					{
						this->parameters.position.x.set(position[0]);
						this->parameters.position.y.set(position[1]);
					}
				}

				// polar to axes
				{
					auto axes = this->polarToAxes(polar);

					// cyclical
					if (this->parameters.axes.cyclic) {
						axes = this->findClosestAxesCycle(axes);
					}

					{
						this->parameters.axes.a.set(axes[0]);
						this->parameters.axes.b.set(axes[1]);
					}
				}
				break;
			}
			case LeadingControl::Axes:
			{
				auto axes = this->getAxes();

				// axes to polar
				auto polar = this->axesToPolar(axes);
				{
					this->parameters.polar.r.set(polar[0]);
					this->parameters.polar.theta.set(polar[1]);
				}

				// polar to position
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

			// alias the axis values
			{
				{
					auto priorValue = this->parameters.axes.a.get();
					auto roundedValue = this->stepsToAxis(
						this->axisToSteps(
							priorValue
							, 0
						)
						, 0);

					if (priorValue != roundedValue) {
						this->parameters.axes.a.set(roundedValue);
					}
				}

				{
					auto priorValue = this->parameters.axes.b.get();
					auto roundedValue = this->stepsToAxis(
						this->axisToSteps(
							priorValue
							, 1
						)
						, 1);

					if (priorValue != roundedValue) {
						this->parameters.axes.b.set(roundedValue);
					}
				}
			}

			// Check if needs push
			{
				bool needsSend = false;
				{
					// Periodically send
					if (this->parameters.axes.sendPeriodically) {
						auto now = chrono::system_clock::now();
						if (now - this->cachedSentValues.lastUpdateRequest > this->cachedSentValues.updatePeriod) {
							needsSend = true;
						}
					}

					// Send if stale
					if (this->parameters.axes.a != this->cachedSentValues.a
						|| this->parameters.axes.b != this->cachedSentValues.b) {
						needsSend = true;
					}

					// Don't send if rs485 is closed
					if (!this->portal->isRS485Open()) {
						needsSend = false;
					}
				}
				if (needsSend) {
					this->pushLazy();
				}
			}

			// Update live axis values
			{
				for (int i = 0; i < 2; i++) {
					auto axis = this->portal->getAxis(i);

					if (axis->getMotionControl()->getCurrentPositionKnown()) {
						this->liveAxisValues[i] = this->stepsToAxis(
							axis->getMotionControl()->getCurrentPosition()
							, i
						);
						this->liveAxisValuesKnown[i] = true;
					}

					if (axis->getMotionControl()->getTargetPositionKnown()) {
						this->liveAxisTargetValues[i] = this->stepsToAxis(
							axis->getMotionControl()->getTargetPosition()
							, i
						);
						this->liveAxisTargetValuesKnown[i] = true;
					}
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
				this->resetPosition();
				}, 'r');
			inspector->addButton("Unwind", [this]() {
				this->unwind();
				}, 'u');
			inspector->addButton("Push", [this]() {
				this->push();
				});
			inspector->addButton("Push lazy", [this]() {
				this->pushLazy();
				}, 'm');
			inspector->addButton("Poll position", [this]() {
				this->pollPosition();
				}, 'p');
			inspector->addButton("Take current position", [this]() {
				this->takeCurrentPosition();
				}, 't');
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
				auto power = 0.4f;

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
							ofDrawLine(-1, 0, 1, 0);
							ofDrawLine(0, -1, 0, 1);

							// Draw bolder lines
							ofSetColor(200);
							ofDrawCircle(0, 0, pow(0.5, power));
							ofDrawCircle(0, 0, 1.0);

							// Draw the overflow line -> where the polar cycle ends (note it's offset by 0.5 in thetaNorm, hence left not right)
							ofDrawLine(-1, 0, 0, 0);
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

					// Draw current position (presumes known)
					{
						auto currentPolar = this->axesToPolar(this->liveAxisValues);
						auto currentPositionInView = polarToView(currentPolar);
						ofPushStyle();
						{
							ofSetColor(100, 100, 200);
							ofDrawCircle(currentPositionInView, 5);
						}
						ofPopStyle();
					}

					// Draw current target position (presumes known)
					{
						auto currentPolar = this->axesToPolar(this->liveAxisTargetValues);
						auto currentPositionInView = polarToView(currentPolar);
						ofPushStyle();
						{
							ofSetColor(100, 100, 200);
							ofNoFill();
							ofDrawCircle(currentPositionInView, 8);
						}
						ofPopStyle();
					}
				};
				panel->onMouse += [this, panelWeak, power](ofxCvGui::MouseArguments& args)
				{
					auto panel = panelWeak.lock();

					args.takeMousePress(panel);

					auto panelToPosition = [panel, power](const glm::vec2& panelPosition) {
						auto panelSize = min(panel->getWidth(), panel->getHeight());
						auto panelCenter = glm::vec2(panel->getWidth() / 2.0f, panel->getHeight() / 2.0f);
						auto xyPanel = glm::vec2({
							ofMap(panelPosition.x
								, panelCenter.x - panelSize / 2
								, panelCenter.x + panelSize / 2
								, -1
								, 1)
							, ofMap(panelPosition.y
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
						return glm::vec2{ x, y };
					};

					auto positionToPanel = [this, panel, power](const glm::vec2& position) {
						auto panelSize = min(panel->getWidth(), panel->getHeight());
						auto panelCenter = glm::vec2(panel->getWidth() / 2.0f, panel->getHeight() / 2.0f);

						auto polar = this->positionToPolar(position);
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

					if (args.isDragging(panel)) {
						auto priorPanelXY = positionToPanel(this->getPosition());
						auto newPanelXY = priorPanelXY + args.movement;
						auto newPosition = panelToPosition(newPanelXY);
						this->setPosition(newPosition);
					}

					if (args.isDoubleClicked(panel)) {
						this->setPosition(panelToPosition(args.local));
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
						horizontalStrip->setCellSizes({ -1, 80 });
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
										const auto r = 0.8f;
										ofDrawCircle(0, 0, 1.0f);
										ofDrawLine(-1, 0, -r, 0);
										ofDrawLine(1, 0, r, 0);
										ofDrawLine(0, -1, 0, -r);
										ofDrawLine(0, 1, 0, r);
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

									ofPushStyle();
									{
										// Axis label in the center
										ofRectangle textBounds(panelCenter - glm::vec2(20, 20), 40, 40);
										ofxCvGui::Utils::drawText(axis.getName(), textBounds);

										// Target position TL
										ofxCvGui::Utils::drawText(ofToString(axis.get(), 3), 20, 20);

										// Current position
										ofxCvGui::Utils::drawText(ofToString(this->liveAxisValues[axisIndex], 3)
											, 20
											, 40
											, true
											, 15
											, 100
											, false
											, ofColor(100, 100, 200));
									}
									ofPopStyle();
								}

								// Draw current value
								{
									auto drawPosition = axisValueToPanelPosition(this->liveAxisValues[axisIndex], 0.5f);
									ofPushStyle();
									{
										ofSetColor(100, 100, 200);
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
										ofSetColor(100, 100, 200);
										ofDrawCircle({ drawPosition.x, drawPosition.y }, 8.0f);
									}
									ofPopStyle();
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
			Pilot::seeThrough()
		{
			this->parameters.axes.a.set(0.0f);
			this->parameters.axes.b.set(0.5f);
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
		void
			Pilot::resetPosition()
		{
			// Zero the polar for the sake of the cyclcical function
			this->setPolar({ 0.0f, 0.0f });

			// Reset the position
			this->setPosition({ 0.0f, 0.0f });
		}

		//----------
		void
			Pilot::unwind()
		{
			auto axes = this->getPolar();
			if (axes[0] > 0) {
				axes[0] -= ceil(axes[0]);
			}
			else {
				axes[0] += ceil(-axes[0]);
			}
			if (axes[1] > 0) {
				axes[1] -= ceil(axes[1]);
			}
			else {
				axes[1] += ceil(-axes[1]);
			}

			this->setAxes(axes);
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

			// axes norm coordinates are offset by half rotation from polar
			// (for axes, left = 0; for polar, right = 0)
			const auto thetaNorm = theta / TWO_PI - 0.5f;

			const auto offset = this->parameters.axes.offset.get();

			// our special sauce for our lenses
			return {
				thetaNorm - (1 - r) * 0.25 + 0.5 - offset
				, thetaNorm + (1 - r) * 0.25  + 0.5 + offset
			};
		}

		//----------
		// https://www.wolframalpha.com/input?i=systems+of+equations+calculator&assumption=%7B%22F%22%2C+%22SolveSystemOf2EquationsCalculator%22%2C+%22equation1%22%7D+-%3E%22a+%3D+t+-+%281-r%29*0.25+%2B+0.5%22&assumption=%22FSelect%22+-%3E+%7B%7B%22SolveSystemOf2EquationsCalculator%22%7D%7D&assumption=%7B%22F%22%2C+%22SolveSystemOf2EquationsCalculator%22%2C+%22equation2%22%7D+-%3E%22b+%3D+t+%2B+%281-r%29*0.25+%2B+0.5%22
		glm::vec2
			Pilot::axesToPolar(const glm::vec2& axes) const
		{
			const auto offset = this->parameters.axes.offset.get();

			auto a = axes[0] + offset;
			auto b = axes[1] - offset;

			// ignore cycles
			auto flattenCycle = [](float x) {
				// bring it into -1...1
				x = fmodf(x, 1);
				if (x < 0) {
					x += 1;
				}
				return x;
			};

			a = flattenCycle(a);
			b = flattenCycle(b);

			auto r = 2 * a - 2 * b + 1;
			auto thetaNorm = (a + b - 1) / 2;

			// Somehow this seems to work
			if (r > 1.0f) {
				r = 1 - (r - 1);
			}
			if (r < 0.0f) {
				thetaNorm += 0.5f;
				r = -r;
			}

			auto theta = (thetaNorm + 0.5f) * TWO_PI;

			return {
				r
				, theta
			};
		}

		//----------
		glm::vec2
			Pilot::findClosestAxesCycle(const glm::vec2& newAxes) const
		{
			const auto currentAxes = this->getAxes();
			
			auto deltaAxes = newAxes - currentAxes;

			while (deltaAxes[0] > 0.5) {
				deltaAxes[0] -= 1.0f;
			}
			while (deltaAxes[0] < -0.5) {
				deltaAxes[0] += 1.0f;
			}

			while (deltaAxes[1] > 0.5) {
				deltaAxes[1] -= 1.0f;
			}
			while (deltaAxes[1] < -0.5) {
				deltaAxes[1] += 1.0f;
			}
			
			return currentAxes + deltaAxes;
		}

		//----------
		Steps
			Pilot::axisToSteps(float axisValue, int axisIndex) const
		{
			float invert = 1.0f;
			if (axisIndex == 1) {
				invert = -1.0f;
			}

			return ofMap(axisValue
				, 0
				, 1
				, 0
				, invert * this->parameters.axes.microstepsPerPrismRotation.get());
		}

		//----------
		float
			Pilot::stepsToAxis(Steps stepsValue, int axisIndex) const
		{
			float invert = 1.0f;
			if (axisIndex == 1) {
				invert = -1.0f;
			}
			return ofMap(stepsValue
				, 0
				, invert * this->parameters.axes.microstepsPerPrismRotation.get()
				, 0
				, 1);
		}

		//----------
		void
			Pilot::push()
		{
			Steps stepsA = this->axisToSteps(this->parameters.axes.a.get(), 0);
			Steps stepsB = this->axisToSteps(this->parameters.axes.b.get(), 1);

			auto message = MsgPack::object{
				{
					"m"
					, MsgPack::array{
						(int32_t)stepsA
						, (int32_t)stepsB
					}
				}
			};

			this->portal->sendToPortal(message, "m");

			this->cachedSentValues.a = this->parameters.axes.a.get();
			this->cachedSentValues.b = this->parameters.axes.b.get();
			this->cachedSentValues.lastUpdateRequest = chrono::system_clock::now();
		}

		//----------
		void
			Pilot::pushLazy()
		{
			this->portal->sendToPortal([this]() {
				Steps stepsA = this->axisToSteps(this->parameters.axes.a.get(), 0);
				Steps stepsB = this->axisToSteps(this->parameters.axes.b.get(), 1);

				auto message = MsgPack::object{
					{
						"m"
						, MsgPack::array{
							(int32_t)stepsA
							, (int32_t)stepsB
						}
					}
				};

				this->cachedSentValues.a = this->parameters.axes.a.get();
				this->cachedSentValues.b = this->parameters.axes.b.get();
				this->cachedSentValues.lastUpdateRequest = chrono::system_clock::now();

				return message;
			}, "m");
		}

		//----------
		void
			Pilot::pollPosition()
		{
			auto message = MsgPack::object{
				{
					"p"
					, MsgPack()
				}
			};

			this->portal->sendToPortal(message, "p");
		}

		//----------
		glm::vec2
			Pilot::getLivePosition() const
		{
			auto axisValues = glm::vec2 {
				this->liveAxisValues[0]
				, this->liveAxisValues[1]
			};
			return this->polarToPosition(this->axesToPolar(axisValues));
		}

		//----------
		glm::vec2
			Pilot::getLiveTargetPosition() const
		{
			auto axisValues = glm::vec2{
				this->liveAxisTargetValues[0]
				, this->liveAxisTargetValues[1]
			};
			return this->polarToPosition(this->axesToPolar(axisValues));
		}

		//----------
		bool
			Pilot::isInTargetPosition() const
		{
			// Check if we've received data first
			if (!(glm::all(this->liveAxisValuesKnown) && glm::all(this->liveAxisTargetValuesKnown))) {
				return false;
			}

			// target on module == target here
			// live value == target here
			// we perform in steps to avoid rounding errors
			return this->axisToSteps(this->liveAxisTargetValues[0], 0) == this->axisToSteps(this->liveAxisValues[0], 0)
				&& this->axisToSteps(this->liveAxisTargetValues[1], 1) == this->axisToSteps(this->liveAxisValues[1], 1)
				&& this->axisToSteps(this->parameters.axes.a.get(), 0) == this->axisToSteps(this->liveAxisValues[0], 0)
				&& this->axisToSteps(this->parameters.axes.b.get(), 1) == this->axisToSteps(this->liveAxisValues[1], 1);
		}

		//----------
		void
			Pilot::takeCurrentPosition()
		{
			this->setAxes(this->liveAxisValues);
		}
	}
}
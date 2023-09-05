#include "pch_App.h"
#include "Routes.h"
#include "../Modules/App.h"

using namespace Modules;

namespace OSC {
	//----------
	vector<Route> routes;

	//----------
	void performOnAllPortals(App * app, std::function<void(shared_ptr<Portal>)> action)
	{
		auto columns = app->getAllColumns();
		for (auto column : columns) {
			auto portals = column->getAllPortals();
			for (auto portal : portals) {
				action(portal);
			}
		}
	}

	//----------
	void initRoutes(Modules::App * app)
	{
		routes = {
			Route {
				"/move"
				, [app](const ofxOscMessage& message) {
					if (message.getNumArgs() < 4) {
						return;
					}

					auto column_id = message.getArgAsInt(0);
					auto portal_id = message.getArgAsInt(1);
					auto x = message.getArgAsFloat(2);
					auto y = message.getArgAsFloat(3);

					auto column = app->getColumnByID(column_id);
					if (!column) {
						throw(Exception("Column " + ofToString(column_id) + " not found"));
					}

					auto portal = column->getPortalByTargetID(portal_id);
					if (!portal) {
						throw(Exception("Portal " + ofToString(portal_id) + " not found"));
					}

					portal->getPilot()->setPosition({ x, y });

					return;
				}
			}
			, Route {
				"/unwind"
				, [app](const ofxOscMessage& message) {
					auto performOnColumn = [](shared_ptr<Column> column) {
						auto portals = column->getAllPortals();
						for (auto portal : portals) {
							portal->getPilot()->unwind();
						}
					};

					if (message.getNumArgs() < 1) {
						// Peform on all
						auto columns = app->getAllColumns();
						for (auto column : columns) {
							performOnColumn(column);
						}
					}
					else if (message.getNumArgs() < 2) {
						auto column_id = message.getArgAsInt(0);

						// Perform on single column
						auto column = app->getColumnByID(column_id);

						if (!column) {
							throw(Exception("Column " + ofToString(column_id) + " not found"));
						}

						performOnColumn(column);
					}
					else {
						// Perform on single column, single portal
						auto column_id = message.getArgAsInt(0);
						auto portal_id = message.getArgAsInt(1);

						auto column = app->getColumnByID(column_id);

						if (!column) {
							throw(Exception("Column " + ofToString(column_id) + " not found"));
						}

						auto portal = column->getPortalByTargetID(portal_id);
						if (!portal) {
							throw(Exception("Portal " + ofToString(portal_id) + " not found"));
						}

						portal->getPilot()->unwind();
					}
				}
			},
			Route {
				"/grid"
				, [app](const ofxOscMessage& message) {
					vector<glm::vec2> positions;
					auto count = message.getNumArgs() / 2;
					for (int i = 0; i < count; i++) {
						positions.push_back({
							message.getArgAsFloat(i * 2 + 0)
							, message.getArgAsFloat(i * 2 + 1)
							});
					}

					app->moveGrid(positions);
				}
			},
			Route {
				"/gridRow"
				, [app](const ofxOscMessage& message) {
					vector<glm::vec2> positions;
					auto count = (message.getNumArgs() - 1) / 2;
					for (int i = 0; i < count; i++) {
						positions.push_back({
							message.getArgAsFloat(i * 2 + 0)
							, message.getArgAsFloat(i * 2 + 1)
							});
					}
					auto rowIndex = (int) message.getArgAsFloat(message.getNumArgs() - 1);
					app->moveGridRow(positions, rowIndex);
				}
			},
			Route {
				"/motionProfile"
				, [app](const ofxOscMessage& message) {
					if (message.getNumArgs() == 1) {
						auto maxVelocity = message.getArgAsInt(0);

						performOnAllPortals(app, [maxVelocity](shared_ptr<Portal> portal) {
							portal->getAxis(0)->getMotionControl()->pushMotionProfile(maxVelocity);
							portal->getAxis(1)->getMotionControl()->pushMotionProfile(maxVelocity);
							});
					}
					else if (message.getNumArgs() >= 2) {
						auto maxVelocity = message.getArgAsInt(0);
						auto acceleration = message.getArgAsInt(1);

						performOnAllPortals(app, [maxVelocity, acceleration](shared_ptr<Portal> portal) {
							portal->getAxis(0)->getMotionControl()->pushMotionProfile(maxVelocity, acceleration);
							portal->getAxis(1)->getMotionControl()->pushMotionProfile(maxVelocity, acceleration);
							});
					}
				}
			},
			Route{
				"/setCurrent"
				, [app](const ofxOscMessage& message) {
					if (message.getNumArgs() >= 1) {
						auto current = message.getArgAsFloat(0);

						performOnAllPortals(app, [&](shared_ptr<Portal> portal) {
							portal->getMotorDriverSettings()->setCurrent(current);
							});
					}
				}
			}
		};
	}

	//----------
	void handleRoute(const ofxOscMessage& message)
	{
		for (const auto& route : routes) {
			if (ofToLower(route.address) == ofToLower(message.getAddress())) {
				route.action(message);
				return;
			}
		}

		ofLogWarning("OSC") << "No route found for " + message.getAddress();
	}
}
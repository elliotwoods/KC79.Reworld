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
		auto installation = app->getInstallation();
		auto columns = installation->getAllColumns();
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

					auto installation = app->getInstallation();
					auto column = installation->getColumnByID(column_id);
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
						auto installation = app->getInstallation();
						auto columns = installation->getAllColumns();
						for (auto column : columns) {
							performOnColumn(column);
						}
					}
					else if (message.getNumArgs() < 2) {
						auto column_id = message.getArgAsInt(0);

						// Perform on single column
						auto installation = app->getInstallation();
						auto column = installation->getColumnByID(column_id);

						if (!column) {
							throw(Exception("Column " + ofToString(column_id) + " not found"));
						}

						performOnColumn(column);
					}
					else {
						// Perform on single column, single portal
						auto column_id = message.getArgAsInt(0);
						auto portal_id = message.getArgAsInt(1);

						auto installation = app->getInstallation();
						auto column = installation->getColumnByID(column_id);

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
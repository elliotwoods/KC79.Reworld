#include "pch_App.h"
#include "Routes.h"
#include "../Modules/App.h"

namespace OSC {
	//----------
	vector<Route> routes;

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
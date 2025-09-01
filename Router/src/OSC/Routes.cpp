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
	void initRoutes(Modules::App* app)
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
			},
			Route{
				"/homeAndZeroLocal"
				, [app](const ofxOscMessage& message) {
					app->getInstallation()->homeHardwareAndZeroPositions();
				}
			},
			Route{
				"/disableLights"
				, [app](const ofxOscMessage& message) {
					app->getInstallation()->broadcast(msgpack11::MsgPack::object {
						{
							"debugLightsEnabled", false
						}
					}, false);
				}
			},
			Route{
				"/axesMoveBlock"
				, [app](const ofxOscMessage& message) {
					// Basic message check
					if (message.getNumArgs() < 4) {
						throw(Exception("Not enough arguments"));
					}
					if (!(message.getArgType(3) == ofxOscArgType::OFXOSC_TYPE_INT32 || message.getArgType(3) == ofxOscArgType::OFXOSC_TYPE_INT64)) {
						throw(Exception("Please sent int (col begin), int (col end), int (portal begin 0-indexed), int (portal end)"));
					}

					auto installation = app->getInstallation();
					auto columns = installation->getAllColumns();

					// This will denote where the data starts in the message
					size_t dataSize = message.getNumArgs() / 4;
					size_t columnIndexBegin = message.getArgAsInt(0);
					size_t columnIndexEnd = message.getArgAsInt(1);
					size_t portalIndexBegin = message.getArgAsInt(2); // Note : we will use 0-indexed portals here
					size_t portalIndexEnd = message.getArgAsInt(3);

					if (dataSize < (columnIndexEnd - columnIndexBegin) * (portalIndexEnd - portalIndexBegin)) {
						throw(Exception("Not enough data in message for columns and portal selected"));
					}

					size_t dataIndexOffset = 4;

					for (size_t columnIndex = columnIndexBegin; columnIndex < columnIndexEnd; columnIndex++) {
						if (columnIndex >= columns.size()) {
							// we are sending more data than columns we have here
							break;
						}

						auto column = columns[columnIndex];
						auto portals = column->getAllPortals();
						for (size_t portalIndex = portalIndexBegin; portalIndex < portalIndexEnd; portalIndex++) {
							if (portalIndex >= portals.size()) {
								// We are receiving data for more portals than exist in this column
								break;
							}

							auto axis1 = message.getArgAsFloat(dataIndexOffset++);
							auto axis2 = message.getArgAsFloat(dataIndexOffset++);

							auto portal = portals[portalIndex];
							portal->getPilot()->setAxesCyclic({
								axis1
								, axis2
								});
						}
					}
				}
			},
			Route{
				"/axesMoveByInidices"
				, [app](const ofxOscMessage& message) {
					size_t dataIndexOffset = 0;

					auto installation = app->getInstallation();
					auto columns = installation->getAllColumns();

					auto messageCount = message.getNumArgs() / 4;
					for (int messageIndex = 0; messageIndex < messageCount; messageIndex++) {
						auto columnIndex = message.getArgAsInt(dataIndexOffset++);
						auto portalIndex = message.getArgAsInt(dataIndexOffset++);

						if (columnIndex >= columns.size()) {
							// outside range
							continue;
						}

						auto portals = columns[columnIndex]->getAllPortals();
						if (portalIndex >= portals.size()) {
							// outside range
							continue;
						}

						auto axis1 = message.getArgAsFloat(dataIndexOffset++);
						auto axis2 = message.getArgAsFloat(dataIndexOffset++);

						auto portal = portals[portalIndex];
						portal->getPilot()->setAxesCyclic({
							axis1
							, axis2
							});
					}
				}
			}
		};
	}

	//----------
	void handleRoute(const ofxOscMessage& message)
	{
		// Handle global routes
		for (const auto& route : routes) {
			if (ofToLower(route.address) == ofToLower(message.getAddress())) {
				route.action(message);
				return;
			}
		}

		// Handle per-column and per-module routes
		{
			auto isInteger = [](const std::string& s) -> bool {
				auto intString = ofToString(ofToInt(s));
				return intString == s;
				};

			auto address = message.getAddress();
			auto addressParts = ofSplitString(address, "/", true, true);
			
			if (addressParts.size() == 1) {
				// Perform on installation
				auto action = Portal::getActionByOSCAddress(addressParts[0]);
				if (action) {
					App::X()->getInstallation()->broadcastAction(action);
				}
			}
			else if (addressParts.size() >= 2) {
				auto hasColumnIndex = isInteger(addressParts[0]);
				auto hasPortalIndex = isInteger(addressParts[1]);
	
				if (!hasColumnIndex && !hasPortalIndex) {
					// Perform on installation
					auto action = Portal::getActionByOSCAddress(addressParts[0]);
					if (action) {
						App::X()->getInstallation()->broadcastAction(action);
					}
				}
				else if (hasColumnIndex && !hasPortalIndex) {
					// Perform on column
					auto columnIndex = ofToInt(addressParts[0]);
					auto column = App::X()->getInstallation()->getColumnByID(columnIndex);
					
					if (column) {
						auto action = Portal::getActionByOSCAddress(addressParts[1]);
						if (action) {
							column->broadcastAction(action);
						}
					}
				}
				else if(hasColumnIndex && hasPortalIndex) {
					// Perform on portal
					auto columnIndex = ofToInt(addressParts[0]);
					auto portalIndex = ofToInt(addressParts[1]);

					auto portal = App::X()->getInstallation()->getPortalByTargetID(columnIndex, portalIndex);
					if (portal) {
						auto action = Portal::getActionByOSCAddress(addressParts[2]);
						if (action) {
							portal->performAction(action);
						}
					}
				}
			}
		}

		ofLogWarning("OSC") << "No route found for " + message.getAddress();
	}
}
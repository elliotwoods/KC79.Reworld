#pragma once

#include "ofxOscMessage.h"
#include "ofMain.h"

namespace Modules {
	class App;
}

namespace OSC {
	struct Exception : public std::exception {
		Exception(const string& message)
			: message(message)
		{ }

		const char* what() const override
		{
			return message.c_str();
		}

		const string message;
	};

	struct Route {
		typedef std::function<void(const ofxOscMessage&)> Action;
		string address;
		Action action;
	};

	extern vector<Route> routes;
	void initRoutes(Modules::App*);
	void handleRoute(const ofxOscMessage&);
}
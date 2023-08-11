#pragma once

#include "ofMain.h"
#include "ofxCvGui.h"
#include "Modules/App.h"

class ofApp : public ofBaseApp {

	public:
		void setup();
		void update();
		void dragEvent(ofDragInfo dragInfo) override;

		ofxCvGui::Builder gui;

		shared_ptr<Modules::App> app;

};

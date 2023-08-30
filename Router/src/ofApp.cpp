#include "pch_App.h"
#include "ofApp.h"
#include "SerialDevices/Factory.h"

//--------------------------------------------------------------
void ofApp::setup(){
	ofSetFrameRate(60.0f);

	gui.init();
	gui.addInspector()->setCaption("Router");
	
	// Register factories
	SerialDevices::registerFactories();

	this->app = make_shared<Modules::App>();
	this->app->init();

	ofxCvGui::inspect(this->app);
}

//--------------------------------------------------------------
void ofApp::update(){
	this->app->update();
}

//--------------------------------------------------------------
void ofApp::dragEvent(ofDragInfo dragInfo) {
	this->app->dragEvent(dragInfo);
}

#include "pch_App.h"
#include "ofApp.h"

#include "SerialDevices/Factory.h"
#include "OSC/Routes.h"

//--------------------------------------------------------------
void ofApp::setup(){
	ofSetFrameRate(60.0f);

	gui.init();
	gui.addInspector()->setCaption("Router");
	
	// Register factories
	SerialDevices::registerFactories();

	this->app = make_shared<Modules::App>();
	this->app->init();

	// Register OSC routes
	OSC::initRoutes(this->app.get());

	ofxCvGui::inspect(this->app);
}

//--------------------------------------------------------------
void ofApp::update(){
	this->app->update();
}

//--------------------------------------------------------------
void ofApp::dragEvent(ofDragInfo dragInfo) {

}

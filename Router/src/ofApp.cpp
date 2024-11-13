#include "pch_App.h"
#include "ofApp.h"

#include "SerialDevices/Factory.h"
#include "Modules/Image/Sources/Factory.h"
#include "OSC/Routes.h"

//--------------------------------------------------------------
void ofApp::setup() {
	ofSetFrameRate(60.0f);

	// Position in screen center
	ofSetWindowPosition((ofGetScreenWidth() - ofGetWidth()) / 2, (ofGetScreenHeight() - ofGetHeight()) / 2);

	ofSetWindowTitle("Router");
	
	// Register factories
	SerialDevices::registerFactories();
	Modules::Image::Sources::registerFactories();

	this->app = Modules::App::X();
	this->app->init();

	gui.init();
	this->app->initGUI(this->gui);

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

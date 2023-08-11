#include "pch_App.h"
#include "ofApp.h"

//--------------------------------------------------------------
void ofApp::setup(){
	gui.init();
	gui.addInspector();
	
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

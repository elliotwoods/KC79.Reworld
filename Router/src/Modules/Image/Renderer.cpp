#include "pch_App.h"
#include "Renderer.h"

namespace Modules {
	namespace Image {
		//----------
		Renderer::Renderer()
		{
		}

		//----------
		string
			Renderer::getTypeName() const
		{
			return "Image::Renderer";
		}

		//----------
		void
			Renderer::init()
		{
		}

		//----------
		void
			Renderer::update()
		{

		}

		//----------
		void
			Renderer::populateInspector(ofxCvGui::InspectArguments& args)
		{

		}

		//----------
		void
			Renderer::deserialise(const nlohmann::json& json)
		{

		}

		//----------
		ofxCvGui::PanelPtr
			Renderer::getMiniView()
		{
			auto view = make_shared<ofxCvGui::Panels::Base>();
			view->onDraw += [this](ofxCvGui::DrawArguments& args) {
				ofxCvGui::Utils::drawText(this->getName(), args.localBounds);
			};
			return view;
		}
	}
}
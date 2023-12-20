#include "pch_App.h"
#include "Swing.h"

namespace Modules {
	namespace Patterns {
		//----------
		void
		Swing::init()
		{
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
			};
		}

		//----------
		string
			Swing::getTypeName() const
		{
			return "Swing";
		}

		//----------
		string
			Swing::getGlyph() const
		{
			return u8"\uf021";
		}

		//----------
		void
			Swing::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;
			inspector->addParameterGroup(this->parameters);
		}

		//----------
		glm::vec2
			Swing::calculate(glm::vec2& positionSNorm)
		{
			auto r = this->getAmplitude();
			auto theta = this->getRotationRad();

			return {
				r * cos(theta)
				, r * sin(theta)
			};
		}

		//----------
		float
			Swing::getAmplitude() const
		{
			// 0 -> 1 -> 0
			return 1.0f - cos(this->phase * this->parameters.amplitude.frequency.get() * TWO_PI);
		}

		//----------
		float
			Swing::getRotationRad() const
		{
			// 0 -> TWO_PI
			return this->phase * this->parameters.rotation.frequency.get();
		}
	}
}
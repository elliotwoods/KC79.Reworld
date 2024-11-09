#include "pch_App.h"
#include "Lens.h"

namespace Modules {
	namespace Patterns {
		//----------
		void
			Lens::init()
		{
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
			};
		}

		//----------
		string
			Lens::getTypeName() const
		{
			return "Lens";
		}

		//----------
		string
			Lens::getGlyph() const
		{
			return u8"\uf31e";
		}

		//----------
		void
			Lens::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;
			inspector->addParameterGroup(this->parameters);
		}

		//----------
		glm::vec2
			Lens::calculate(glm::vec2& positionSNorm)
		{
			// calculate r factor with power applied
			auto r = glm::length(positionSNorm);
			r = pow(r, this->parameters.power.get());

			if (r < 1e-7) {
				return { 0, 0 };
			}

			// calculate amplitude at this point 
			auto amplitude = r * this->getAmplitude();

			auto direction = glm::normalize(positionSNorm);

			return amplitude * direction;
		}

		//----------
		float
			Lens::getAmplitude() const
		{
			// 0 -> 1 -> 0 -> -1 - > 0
			return sin(this->phase * this->parameters.amplitude.frequency.get() * TWO_PI);
		}
	}
}
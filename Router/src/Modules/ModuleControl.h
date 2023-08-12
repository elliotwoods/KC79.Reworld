#pragma once

#include "Base.h"
#include "RS485.h"

namespace Modules {
	class ModuleControl : public Base
	{
	public:
		enum Axis {
			A
			, B
		};

		ModuleControl(shared_ptr<RS485>);

		string getTypeName() const override;
		void init() override;
		void update() override;

		void populateInspector(ofxCvGui::InspectArguments&);

		void setCurrent(RS485::Target target, float);
		void runTestRoutine(RS485::Target target, Axis);
	protected:
		weak_ptr<RS485> rs485;

		struct : ofParameterGroup {
			struct : ofParameterGroup {
				ofParameter<int> targetID{ "Target ID", 16 };
				ofParameter<float> current{ "Current", 0.1f, 0.0f, 0.3f };
				PARAM_DECLARE("Debug", targetID, current);
			} debug;
			PARAM_DECLARE("ModuleControl", debug)
		} parameters;

		struct {
			struct {
				struct {
					float current = -1;
				} motorDriverSettings;
			} cachedValues;
		} debug;
	};
}
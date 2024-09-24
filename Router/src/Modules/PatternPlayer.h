#pragma once

#include "Base.h"
#include "Patterns/Base.h"

namespace Modules {
	class App;

	class PatternPlayer : public Base {
	public:
		PatternPlayer(App *);
		string getTypeName() const override;
		string getGlyph() const override;

		void init() override;
		void update() override;
		void populateInspector(ofxCvGui::InspectArguments&);

		vector<shared_ptr<Patterns::Base>> getActivePatterns() const;

	protected:
		App* app;
		vector<shared_ptr<Patterns::Base>> patterns;

		struct : ofParameterGroup {
			ofParameter<bool> enabled{ "Enabled", true };
			PARAM_DECLARE("PatternPlayer", enabled);
		} parameters;

		int currentPattern = 0;
		float timestampCurrentPatternStarted = 0.0f;
	};
}
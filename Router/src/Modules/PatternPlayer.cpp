#include "pch_App.h"
#include "PatternPlayer.h"
#include "App.h"

#include "Patterns/Swing.h"
#include "Patterns/Lens.h"

namespace Modules {
	//----------
	PatternPlayer::PatternPlayer(App * app)
		: app(app)
	{
		{
			auto pattern = make_shared<Patterns::Swing>();
			pattern->init();
			this->patterns.push_back(pattern);
		}

		{
			auto pattern = make_shared<Patterns::Lens>();
			pattern->init();
			this->patterns.push_back(pattern);
		}
	}

	//----------
	string
		PatternPlayer::getTypeName() const
	{
		return "PatternPlayer";
	}

	//----------
	string
		PatternPlayer::getGlyph() const
	{
		return u8"\uf144";
	}

	//----------
	void
		PatternPlayer::init()
	{
		this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
			this->populateInspector(args);
		};
	}

	//----------
	void
		PatternPlayer::update()
	{
		// Accumulate active patterns
		auto activePatterns = this->getActivePatterns();

		// Check if any patterns available
		if (activePatterns.empty()) {
			return;
		}

		auto now = ofGetElapsedTimef();

		// Check we're not overflowing
		if (this->currentPattern > activePatterns.size()) {
			this->currentPattern = 0;
			this->timestampCurrentPatternStarted = now;
		}

		auto pattern = activePatterns[this->currentPattern];
		auto timeWithinPattern = now - this->timestampCurrentPatternStarted;
		if (pattern->isEnded(timeWithinPattern)) {
			// pattern ended - go to next one
			this->currentPattern++;
			this->currentPattern %= activePatterns.size();
			this->timestampCurrentPatternStarted = now;

			// and set these variables for use now
			pattern = activePatterns[this->currentPattern];
			timeWithinPattern = 0;
		}

		// Set the time (sets the phase variable)
		pattern->setTime(timeWithinPattern);

		// Calculate positions
		vector<glm::vec2> positions;
		{
			auto size = app->getSize();
			for (int j = 0; j < size[1]; j++) {
				for (int i = 0; i < size[0]; i++) {
					auto portalGridPosSNorm = glm::vec2{
						(float)i / (float)(size[0] - 1) * 2.0f - 1.0f
						, (float)j / (float)(size[1] - 1) * 2.0f - 1.0f
					};

					auto portalPosition = pattern->calculate(portalGridPosSNorm);

					positions.push_back(portalPosition);
				}
			}
		}

		// Perform the move
		app->moveGrid(positions);
	}

	//----------
	void
		PatternPlayer::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;
		for (auto pattern : this->patterns) {
			auto menuItem = inspector->addSubMenu(pattern->getName(), pattern);

			// Add the glyph
			auto glyph = pattern->getGlyph();
			if (!glyph.empty()) {
				menuItem->onDraw += [glyph](ofxCvGui::DrawArguments& args) {
					auto bounds = args.localBounds;
					bounds.width = 30;
					bounds.x = 20;
					ofxCvGui::Utils::drawGlyph(glyph, bounds);
				};
			}
			
			// Add an enabled button
			{
				auto button = make_shared<ofxCvGui::Widgets::Toggle>(pattern->baseParameters.enabled);
				button->setDrawGlyph(u8"\uf011");
				button->setWidth(80.0f);
				button->setHeight(menuItem->getHeight() - 20);
				button->setPosition({ 10, 10 });
				menuItem->addChild(button);
			}
		}

		inspector->addParameterGroup(this->parameters);

		inspector->add(App::X()->getPositionsPreview());
	}

	//----------
	vector<shared_ptr<Patterns::Base>>
		PatternPlayer::getActivePatterns() const
	{
		vector<shared_ptr<Patterns::Base>> activePatterns;
		for (auto pattern : this->patterns) {
			if (pattern->baseParameters.enabled.get()) {
				activePatterns.push_back(pattern);
			}
		}
		return activePatterns;
	}

}
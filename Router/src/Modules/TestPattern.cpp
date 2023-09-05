#include "pch_App.h"
#include "TestPattern.h"
#include "App.h"

namespace Modules {
	//----------
	TestPattern::TestPattern(App* app)
		: app(app)
	{

	}

	//----------
	string
		TestPattern::getTypeName() const
	{
		return "TestPattern";
	}

	//----------
	string
		TestPattern::getGlyph() const
	{
		return u8"\ue4f3";
	}

	//----------
	void
		TestPattern::init()
	{
		this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
			this->populateInspector(args);
		};
	}

	//----------
	void
		TestPattern::update()
	{
		if (!this->parameters.enabled) {
			return;
		}

		// check frame number (perform every N frames)
		{
			auto frameIndex = ofGetFrameNum();
			auto everyNFrames = this->parameters.everyNFrames.get();
			if (frameIndex % everyNFrames == 0) {
				// OK
			}
			else {
				// Don't do anything
				return;
			}
		}

		// wave
		this->wave();

		// unwind
		if (this->parameters.unwind.enabled) {
			auto now = chrono::system_clock::now();
			auto period = std::chrono::milliseconds((int)(this->parameters.unwind.period * 1000.0f));
			if (this->lastUnwind - now > period) {
				this->unwind();
			}
		}
	}

	//----------
	void
		TestPattern::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;
		inspector->addParameterGroup(this->parameters);
	}

	//----------
	void
		TestPattern::wave()
	{

		// function to calculate values
		auto calcPosition = [this](glm::vec2 portalGridPosSNorm) {
			auto x = portalGridPosSNorm[0];
			auto y = portalGridPosSNorm[1];

			const auto& width = this->parameters.wave.width;
			const auto& height = this->parameters.wave.height;
			const auto& amplitude = this->parameters.wave.amplitude;

			x /= width;
			y /= height;

			auto t = ofGetElapsedTimef();
			auto phase = t / this->parameters.wave.period.get() * TWO_PI;

			auto value = cos(x * PI) * cos(y * PI + phase);

			// https://www.wolframalpha.com/input?i2d=true&i=%E2%88%87%5C%2840%29cos%5C%2840%29Divide%5Bx%2Cpi%5D%5C%2841%29+*+cos%5C%2840%29Divide%5By%2Cpi%5D%2Bphase%5C%2841%29%5C%2841%29
			auto ddx = -(cos(y * PI + phase) * sin(x * PI)) / PI * amplitude;
			auto ddy = -(cos(x * PI) * sin(y * PI + phase)) / PI * amplitude;

			return glm::vec2{
				ddx
				, ddy
			};
		};

		vector<glm::vec2> positions;

		auto size = app->getSize();
		for (int j = 0; j < size[1]; j++) {
			for (int i = 0; i < size[0]; i++) {
				auto portalGridPosSNorm = glm::vec2{
					(float)i / (float)(size[0] - 1) * 2.0f - 1.0f
					, (float)j / (float)(size[1] - 1) * 2.0f - 1.0f
				};

				auto portalPosition = calcPosition(portalGridPosSNorm);

				positions.push_back(portalPosition);
			}
		}

		app->moveGrid(positions);
	}

	//----------
	void
		TestPattern::unwind()
	{
		auto columns = app->getAllColumns();
		for (auto column : columns) {
			auto portals = column->getAllPortals();
			for (auto portal : portals) {
				portal->getPilot()->unwind();
			}
		}

		this->lastUnwind = chrono::system_clock::now();
	}
}
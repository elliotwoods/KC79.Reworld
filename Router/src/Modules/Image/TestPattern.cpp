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
		TestPattern::deserialise(const nlohmann::json& json)
	{
		if (json.contains("enabled")) {
			this->parameters.enabled.set((bool)json["enabled"]);
		}
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

		// Check the home timer
		if (this->parameters.homeAndZero.timer.enabled) {
			auto now = chrono::system_clock::now();
			const auto period = chrono::milliseconds((uint32_t)(this->parameters.homeAndZero.timer.period_m.get() * 60.0f * 1000.0f));
			const auto duration = chrono::milliseconds((uint32_t)(this->parameters.homeAndZero.timer.duration_s.get() * 1000.0f));
			auto timeSinceLastActiveStart = now - this->lastHomeAndZeroActiveStart;

			if (this->parameters.homeAndZero.active) {
				// Is active, check if we want to de-activate
				auto timeSinceLastActiveStart = now - this->lastHomeAndZeroActiveStart;
				if (timeSinceLastActiveStart >= duration) {
					// deactivate
					this->parameters.homeAndZero.active.set(false);
				}
			}
			else {
				// Is inactive, see if we want to activate
				if (timeSinceLastActiveStart >= period) {
					// deactivate
					this->parameters.homeAndZero.active.set(true);
					this->lastHomeAndZeroActiveStart = now;
				}
			}

			if (this->parameters.homeAndZero.timer.rampAmplitude.enabled) {
				auto timeSinceLastActiveStart_ms = chrono::duration_cast<chrono::milliseconds>(timeSinceLastActiveStart).count();
				auto period_ms = chrono::duration_cast<chrono::milliseconds>(period).count();
				auto timeWithinPeriodNorm = (float)timeSinceLastActiveStart_ms / (float)period_ms;
				auto amplitude = timeWithinPeriodNorm * this->parameters.homeAndZero.timer.rampAmplitude.maximum.get();
				this->parameters.wave.amplitude.set(amplitude);
			}
		}

		if (this->parameters.homeAndZero.active) {
			// Only do home and zero
			this->homeAndZero();
		}
		else {
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
	}

	//----------
	void
		TestPattern::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;
		inspector->addParameterGroup(this->parameters);
		inspector->addButton("Unwind", [this]() {
			this->unwind();
			});
		inspector->addLiveValue<string>("Time since last home and zero active start", [this]() {
			return Utils::durationToString(chrono::system_clock::now() - this->lastHomeAndZeroActiveStart);
			});
	}

	//----------
	void
		TestPattern::wave()
	{
		this->waveData.phase += ofGetLastFrameTime() / this->parameters.wave.period.get() * TWO_PI;

		// function to calculate values
		auto calcPosition = [this](glm::vec2 portalGridPosSNorm) {
			auto x = portalGridPosSNorm[0];
			auto y = portalGridPosSNorm[1];

			const auto& width = this->parameters.wave.width;
			const auto& height = this->parameters.wave.height;
			const auto& amplitude = this->parameters.wave.amplitude;

			x /= width;
			y /= height;

			auto phase = this->waveData.phase;

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

	//----------
	void
		TestPattern::homeAndZero()
	{
		auto columns = app->getAllColumns();
		for (auto column : columns) {
			column->broadcastHome();

			auto portals = column->getAllPortals();
			for (auto portal : portals) {
				portal->getPilot()->setAxes({ 0, 0 });
			}
		}

		this->lastUnwind = chrono::system_clock::now();
	}
}
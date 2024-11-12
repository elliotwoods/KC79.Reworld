#pragma once

#include "ofxCvGui.h"
#include <string>

namespace Modules {
	class Portal;
}

namespace Utils {
	std::string getAxisLetter(int axisIndex);

	struct IsFrameNew {
		void notify() {
			this->incomingEventCount++;
		}

		void update() {
			this->eventCount = incomingEventCount;
			this->isFrameNew = this->eventCount > 0;

			this->incomingEventCount = 0;
		}

		bool isFrameNew = false;
		size_t eventCount = 0;

	protected:
		size_t incomingEventCount = 0;
	};

	struct IReportedState {
		IReportedState(const string& name);
		virtual void processIncoming(const nlohmann::json&) = 0;
		virtual string toString() const = 0;
		const std::string name;
		bool hasBeenReported = false;
	};

	template<typename T>
	struct ReportedState : IReportedState {
		ReportedState(const string& name)
			: IReportedState(name)
		{

		}

		ReportedState(const string& name, function<string(T)> toStringFunction)
			: IReportedState(name)
			, toStringFunction(toStringFunction)
		{

		}

		T value;

		void processIncoming(const nlohmann::json& json) override
		{
			if (json.contains(this->name)) {
				this->value = (T)json[name];
				this->hasBeenReported = true;
			}
		}

		string toString() const override
		{
			if (!this->hasBeenReported) {
				return "[unknown]";
			}
			else {
				if (this->toStringFunction) {
					return this->toStringFunction(this->value);
				}
				else {
					return ofToString(this->value);
				}
			}
		}

		function<string(T)> toStringFunction;
	};

	ofxCvGui::ElementPtr makeGUIElementTyped(ReportedState<bool> *);
	ofxCvGui::ElementPtr makeGUIElementTyped(ReportedState<string>*);
	ofxCvGui::ElementPtr makeGUIElementTyped(ReportedState<float>*);
	ofxCvGui::ElementPtr makeGUIElementTyped(ReportedState<double> *);

	template<typename T, typename std::enable_if<std::is_integral<T>::value, T>::type = 0>
	ofxCvGui::ElementPtr
	makeGUIElementTyped(ReportedState<T>* variable) {
		return make_shared<ofxCvGui::Widgets::LiveValue<T>>(variable->name, [variable]() {
			return variable->value;
			});
	}

	ofxCvGui::ElementPtr makeGUIElement(IReportedState* variable);

	string millisToString(uint32_t millis);
	string durationToString(const chrono::system_clock::duration&);

	template<typename T>
	bool deserialize(const nlohmann::json&, ofParameter<T>&);
}

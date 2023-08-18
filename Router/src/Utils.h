#pragma once

#include <string>

namespace Utils {
	std::string getAxisLetter(int axisIndex);

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
				return ofToString(this->value);
			}
		}
	};
}

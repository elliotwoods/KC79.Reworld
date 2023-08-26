#pragma once

#include <memory>
#include <vector>
#include "Base.h"
#include <U8g2lib.h>
#include "Logger.h"

#define GUI_MAX_LOG_LINES 5

namespace Modules {
	class GUI : public Base, public ILogListener {
	public:
		GUI();
		
		const char * getTypeName() const;
		void setup() override;
		void update() override;

		void onLogMessage(const LogMessage&) override;
	private:
		void draw();
		U8G2 u8g2;

		struct LogLine {
			size_t count;
			LogMessage logMessage;
		};
		std::vector<LogLine> logLines;
		bool guiEnabled = false;
		bool needsUpdate = true;
	};
}


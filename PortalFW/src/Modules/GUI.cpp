#include "GUI.h"
#include <u8g2hal.h>
#include "../Platform/Platform.h"
#include <stdlib.h>

namespace Modules {
	//----------
	GUI::GUI()
	{

	}

	//----------
	const char *
	GUI::getTypeName() const
	{
		return "GUI";
	}

	//----------
	void
	GUI::setup()
	{
		// Initialise the screen
		u8x8_stm32_init_i2c();

		u8g2_Setup_ssd1306_i2c_128x64_noname_f(this->u8g2.getU8g2()
			, U8G2_R2
			, u8x8_byte_stm32_hw_i2c
			, u8x8_stm32_gpio_and_delay);
		
		u8x8_SetI2CAddress(this->u8g2.getU8x8(), 0x3c);
		this->u8g2.begin();

		Logger::X().logListeners.push_back(this);
		this->update();
	}

	//----------
	void
	GUI::update()
	{
		if(!needsUpdate) {
			return;
		}

		// DRAW THE CURRENT PANEL
		this->u8g2.clearBuffer();
		{
			this->draw();
		}
		this->u8g2.sendBuffer();

		this->needsUpdate = false;
	}

	//----------
	void
	GUI::onLogMessage(const LogMessage& logMessage)
	{
		// Check if it's same as last message received
		bool hasBeenAdded = false;
		{
			if(!this->logLines.empty()) {
				auto & lastMessage = this->logLines.back();
				if(lastMessage.logMessage.level == logMessage.level
				&& lastMessage.logMessage.message == logMessage.message) {
					// Last message matches this one
					lastMessage.count++;
					hasBeenAdded = true;
				}
			}
		}

		// Add the log message
		if(!hasBeenAdded) {
			this->logLines.push_back({
				1
				, logMessage
			});
		}

		// Check we don't exceed max count
		if(logLines.size() > GUI_MAX_LOG_LINES) {
			// Truncade to last N elements
			logLines.assign(logLines.end() - GUI_MAX_LOG_LINES
				, logLines.end());
		}

		this->needsUpdate = true;
	}

	//----------
	void
	GUI::draw()
	{
		u8g2.setFont(u8g2_font_6x12_mr);
		const int rowHeight = 12;
		const int textHeight = 10;
		const int textOffset = (rowHeight - textHeight) / 2 + textHeight - 1;
		int y = 0;

		for(auto & logLine : this->logLines) {
			u8g2.drawStr(8, y + textOffset, logLine.logMessage.message.c_str());

			if(logLine.count > 1) {
				u8g2.drawBox(106, y, 28, rowHeight);
				u8g2.setDrawColor(0);
				{
					char message[8];
					itoa(logLine.count, message, 10);
					u8g2.drawStr(108, y + textOffset, message);
				}
				u8g2.setDrawColor(1);
			}

			if(logLine.logMessage.level != LogLevel::Status) {
				switch(logLine.logMessage.level) {
				case LogLevel::Warning:
					u8g2.drawCircle(2, y + rowHeight / 2, 2);
					break;
				case LogLevel::Error:
					u8g2.drawDisc(2, y + rowHeight / 2, 2);
					break;
				default:
					break;
				}
			}

			y += rowHeight;
		}
	}
}

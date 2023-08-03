#include "SplashScreen.h"
#include "Logo.h"
#include "Log.h"

namespace GUI {
	namespace Panels {
		//----------
		SplashScreen::SplashScreen()
		{

		}

		//----------
		void
		SplashScreen::update()
		{
			this->setMessage(Logger::X().getLastMessage());
		}

		//----------
		void
		SplashScreen::draw(U8G2 & u8g2)
		{
			u8g2.drawXBM((128-KimchipsLogo100_width)/2
						//, (64-KimchipsLogo100_height)/2
						, 0
						, KimchipsLogo100_width
						, KimchipsLogo100_height
						, KimchipsLogo100_bits
						); 
			u8g2.setFont(u8g2_font_nerhoe_tr);
			u8g2.drawStr(10, 50, Logger::X().getLastMessage().c_str());
		}
		
		//----------
		void
		SplashScreen::buttonPressed()
		{
			
		}

		//----------
		void
		SplashScreen::dial(int8_t)
		{
			
		}

		//----------
		void
		SplashScreen::setMessage(const std::string& message)
		{
			if(message[0]==' '){
				this->message += message;
			}else{
				this->message = message;
			}
		}
	}
}


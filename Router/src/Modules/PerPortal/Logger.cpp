#include "pch_App.h"
#include "Logger.h"

namespace Modules {
	namespace PerPortal {
#pragma mark MessageElement
		//----------
		Logger::MessageElement::MessageElement(const LogMessage& logMessage)
			: logMessage(logMessage)
		{
			this->onDraw += [this](ofxCvGui::DrawArguments& args) {
				this->draw(args);
			};

			this->onMouse += [this](ofxCvGui::MouseArguments& args) {
				this->mouse(args);
			};
			this->setHeight(40.0f);
			this->setWidth(200.0f);
		}

		//----------
		void
			Logger::MessageElement::draw(ofxCvGui::DrawArguments& args)
		{
			// Draw indicator circle
			{
				switch (this->logMessage.level) {
				case LogLevel::Status:
					break;
				case LogLevel::Warning:
				{
					ofPushStyle();
					{
						ofNoFill();
						ofSetColor(200, 200, 200);
						ofDrawCircle(30, 20, 10);
					}
					ofPopStyle();
					break;
				}
				case LogLevel::Error:
				{
					ofPushStyle();
					{
						ofFill();
						ofSetColor(200, 100, 100);
						ofDrawCircle(30, 20, 10);
					}
					ofPopStyle();
				}
				default:
					break;
				}
			}

			// Draw text
			{
				auto bounds = args.localBounds;
				{
					bounds.x += 50;
					bounds.width -= 50;
				}
				ofxCvGui::Utils::drawText(this->logMessage.message, bounds, false, false);
			}

			// Draw repeat indicator
			if (logMessage.count > 1) {
				auto bounds = args.localBounds;
				{
					bounds.x = bounds.getRight() - 50;
					bounds.width = 50;
				}
				ofxCvGui::Utils::drawText(ofToString(this->logMessage.count), bounds, true, false);
			}

			// Draw timestamp
			if (logMessage.timetamp != 0) {
				auto bounds = args.localBounds;
				bounds.width = 50;
				ofxCvGui::Utils::drawText(ofToString(this->logMessage.timetamp, 2) + "s", bounds, true, false);
			}
		}

		//----------
		void
			Logger::MessageElement::mouse(ofxCvGui::MouseArguments& args)
		{

		}

#pragma mark Logger
		//----------
		Logger::Logger(Portal* portal)
			: portal(portal)
		{

		}

		//----------
		string
			Logger::getTypeName() const
		{
			return "Logger";
		}

		//----------
		string
			Logger::getGlyph() const
		{
			return u8"\uf086";
		}

		//----------
		void
			Logger::init()
		{
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
			};
		}
		
		//----------
		void
			Logger::update()
		{
			// Clear out panels that are not being used
			for (auto it = this->panels.begin(); it != this->panels.end();) {
				auto lock = it->lock();
				if (lock) {
					it++;
				}
				else {
					it = this->panels.erase(it);
				}
			}

			// Limit max message count
			this->limitMaxMessages();

			// Refresh panels if stale
			if(this->panelsStale) {
				this->refreshPanels();
			}

		}

		//----------
		void Logger::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;
			
			inspector->add(this->getPanel());

			inspector->addButton("Clear", [this]() {
				this->clear();
				});
			inspector->addParameterGroup(this->parameters);
		}

		//----------
		ofxCvGui::PanelPtr
			Logger::getPanel()
		{
			auto panel = ofxCvGui::Panels::makeWidgets();
			panel->setHeight(400.0f);
			this->refreshPanel(panel);
			this->panels.push_back(panel);
			return panel;
		}

		//----------
		void
			Logger::processIncoming(const nlohmann::json& json)
		{
			for (const auto& jsonMessage : json) {
				if (jsonMessage.contains("message") && jsonMessage.contains("level")) {
					LogMessage message{
					jsonMessage["message"]
					, (LogLevel) (uint8_t)jsonMessage["level"]
					};

					// Add timestamp if it exists
					if (jsonMessage.contains("timestamp_ms")) {
						message.timetamp = (float)((uint32_t)jsonMessage["timestamp_ms"]) / 1000.0f;
					}

					// Check if should add to existing
					bool foundExisting = false;
					if (!this->logMessages.empty()) {
						auto& lastMessage = this->logMessages.back();
						if (lastMessage.message == message.message
							&& lastMessage.level == message.level) {
							lastMessage.count++;
							foundExisting = true;
						}
					}
					if (!foundExisting) {
						this->logMessages.push_back(message);
					}
				}
			}

			this->panelsStale = true;
		}
		
		//----------
		void
			Logger::limitMaxMessages()
		{
			const auto maxSize = this->parameters.maxHistorySize.get();

			if (this->logMessages.size() <= maxSize) {
				// We're already OK, nothing to do
				return;
			}

			// Presume that we will definitely change panels
			this->panelsStale = true;

			// Firstly try to cut down by truncating non-error messages
			if (false) // actually don't - this code doesn't work and gets trapped in an infinite loop
			{
				for (auto it = this->logMessages.begin(); it != this->logMessages.end(); ) {
					if (it->level == LogLevel::Status) {
						// consider abbreviating
						auto next = it + 1;
						auto itEnd = next;

						// find the end of the sequence of status messages
						size_t count = 1;
						for (; itEnd != this->logMessages.end(); itEnd++) {
							if (itEnd->level != LogLevel::Status) {
								break;
							}
							count++;
						}

						if (itEnd != next) {
							// there was a sequence of status messages
							it = this->logMessages.erase(it, itEnd);

							// add the abbreviated message
							LogMessage abbreviatedMessage{
								"[abbreviated status messages]"
								, LogLevel::Status
								, count
							};

							// insert this abbreviated message in
							it = this->logMessages.insert(it, abbreviatedMessage);
						}

						if (this->logMessages.size() <= maxSize) {
							// Check if now we can exit this routine
							break;
						}
					}
				}
			}


			// Now if we're too long, just truncate the end
			if (this->logMessages.size() > maxSize) {
				auto needsRemoveCount = this->logMessages.size() - maxSize;
				this->logMessages.assign(this->logMessages.begin() + needsRemoveCount, this->logMessages.end());
			}
		}

		//----------
		void
			Logger::clear()
		{
			this->logMessages.clear();
		}
		
		//----------
		const Logger::LogMessage *
			Logger::getLatestMessage() const
		{
			if (this->logMessages.empty()) {
				return nullptr;
			}
			else {
				return &this->logMessages.back();
			}
		}

		//----------
		void
			Logger::refreshPanels()
		{
			for (auto panelWeak : this->panels) {
				auto panel = panelWeak.lock();
				if (panel) {
					this->refreshPanel(panel);
				}
			}
			this->panelsStale = false;
		}

		//----------
		void
			Logger::refreshPanel(shared_ptr<ofxCvGui::Panels::Widgets> panel)
		{
			panel->clear();
			for (auto it = this->logMessages.rbegin()
				; it != this->logMessages.rend()
				; it++) {
				panel->add(make_shared<MessageElement>(*it));
			}
		}
	}
}
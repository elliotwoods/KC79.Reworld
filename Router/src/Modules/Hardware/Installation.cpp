#include "pch_App.h"

#include "Installation.h"
#include "../Utils.h"

using namespace msgpack11;

namespace Modules {
	namespace Hardware {
		//----------
		Installation::Installation()
		{

		}

		//----------
		Installation::~Installation()
		{

		}

		//----------
		string
			Installation::getTypeName() const
		{
			return "Installation";
		}

		//----------
		void
			Installation::init()
		{
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
				};
		}

		//----------
		void
			Installation::update()
		{
			// update modules
			for (const auto& column : this->columns) {
				column.second->update();
			}
		}

		//----------
		void
			Installation::deserialise(const nlohmann::json& json)
		{
			this->columns.clear();

			if (json.contains("columns")) {
				// Build up the columns
				auto jsonColumns = json["columns"];

				for (auto jsonColumn : jsonColumns) {
					// Berge in the commmon settings
					if (json.contains("columnCommonSettings")) {
						const auto& jsonColumnSettings = json["columnCommonSettings"];
						jsonColumn.update(jsonColumnSettings);
					}

					auto column = make_shared<Column>();
					if (jsonColumn.contains("index")) {
						this->columns.emplace((int)jsonColumn["index"], column);
						column->deserialise(jsonColumn);
						column->init();
					}
					else {
						ofLogError("deserialise") << "Config json needs to set an index for each column";
					}
				}
			}
			else {
				// create a blank column
				auto column = make_shared<Column>();
				this->columns.emplace(1, column);
				column->init();
			}

			ofxCvGui::refreshInspector(this);
		}

		//----------
		void
			Installation::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;

			// Actions

			{
				auto buttonStack = inspector->addHorizontalStack();
				// Special action for poll
				buttonStack->addButton("Poll", [this]() {
					this->pollAll();
					}, ' ')->setDrawGlyph(u8"\uf059");

					// Add actions
					auto actions = Portal::getActions();
					for (const auto& action : actions) {
						auto hasHotkey = action.shortcutKey != 0;

						auto buttonAction = [this, action]() {
							this->broadcast(action.message);
							};

						auto button = hasHotkey
							? buttonStack->addButton(action.caption, buttonAction, action.shortcutKey)
							: buttonStack->addButton(action.caption, buttonAction);

						button->setDrawGlyph(action.icon);
					}
			}

			inspector->addSpacer();

			// Add columns
			{
				auto stack = inspector->addHorizontalStack();
				float colHeight = 10.0f;
				for (const auto& it : this->columns) {
					const auto& column = it.second;
					auto columnWeak = weak_ptr<Column>(column);
					auto button = make_shared<ofxCvGui::Widgets::SubMenuInspectable>("", column);

					// Custom draw
					{
						button->onDraw += [columnWeak](ofxCvGui::DrawArguments& args) {
							};
					}

					// Vertical stack of elements in the button
					float height = 0.0f;
					{

						auto verticalStack = make_shared<ofxCvGui::Widgets::VerticalStack>(ofxCvGui::Widgets::VerticalStack::Layout::UseElementHeight);
						{
							button->addChild(verticalStack);
							auto verticalStackWeak = weak_ptr<ofxCvGui::Element>(verticalStack);
							button->onBoundsChange += [verticalStackWeak](ofxCvGui::BoundsChangeArguments& args) {
								auto verticalStack = verticalStackWeak.lock();
								auto bounds = args.localBounds;
								bounds.width -= 60.0f;
								verticalStack->setBounds(bounds);
								};
						}

						// Title
						{
							auto element = make_shared<ofxCvGui::Widgets::Title>(column->getName());
							verticalStack->add(element);
							height += element->getHeight();
						}

						// RS485 connected
						{
							auto element = make_shared<ofxCvGui::Widgets::Indicator>("RS485", [columnWeak]() {
								auto column = columnWeak.lock();
								if (column->getRS485()->isConnected()) {
									return ofxCvGui::Widgets::Indicator::Status::Good;
								}
								else {
									return ofxCvGui::Widgets::Indicator::Status::Warning;
								}
								});
							verticalStack->add(element);
							height += element->getHeight();
						}

						// Outbox size
						{
							auto element = make_shared<ofxCvGui::Widgets::LiveValue<size_t>>("Outbox size", [columnWeak]() {
								auto column = columnWeak.lock();
								return column->getRS485()->getOutboxCount();
								});
							verticalStack->add(element);
							height += element->getHeight();
						}

						// Tx and Rx heartbeats
						{
							auto widgets = column->getRS485()->getWidgets();
							for (auto widget : widgets) {
								verticalStack->add(widget);
								height += widget->getHeight();
							}
						}

						// Preview of all elements
						{
							auto element = column->getMiniView(ofGetWidth() / this->columns.size() - 20.0f);
							verticalStack->add(element);
							height += element->getHeight();
						}

						// Spacer at bottom
						{
							auto element = make_shared<ofxCvGui::Widgets::Spacer>();
							verticalStack->add(element);
							height += element->getHeight();
						}
					}
					button->setHeight(height);
					colHeight = height;

					stack->add(button);
				}

				stack->setHeight(colHeight);
			}
		}

		//----------
		vector<shared_ptr<Column>>
			Installation::getAllColumns() const
		{
			vector<shared_ptr<Column>> columns;
			for (const auto& it : this->columns) {
				columns.push_back(it.second);
			}

			return columns;
		}

		//----------
		shared_ptr<Column>
			Installation::getColumnByID(int id) const
		{
			if (this->columns.find(id) == this->columns.end()) {
				return shared_ptr<Column>();
			}
			else {
				return this->columns.at(id);
			}
		}

		//----------
		vector<shared_ptr<Portal>>
			Installation::getAllPortals() const
		{
			vector<shared_ptr<Portal>> portals;
			for (const auto& it : this->columns) {
				auto column = it.second;
				auto columnPortals = column->getAllPortals();
				portals.insert(portals.end(), columnPortals.begin(), columnPortals.end());
			}

			return portals;
		}

		//----------
		shared_ptr<Portal>
			Installation::getPortalByTargetID(int columnID, Portal::Target target) const
		{
			auto column = this->getColumnByID(columnID);
			if (!column) {
				return shared_ptr<Portal>();
			}

			return column->getPortalByTargetID(target);
		}

		//----------
		glm::tvec2<size_t>
			Installation::getResolution() const
		{
			auto allColumns = this->getAllColumns();
			auto testColumn = allColumns.front();
			auto testPortals = testColumn->getAllPortals();

			return {
				this->columns.size() * testColumn->getCountX()
				, testColumn->getCountY()
			};
		}

		//----------
		void
			Installation::pollAll()
		{
			for (const auto& it : this->columns) {
				auto column = it.second;
				column->pollAll();
			}
		}

		//----------
		void
			Installation::broadcast(const msgpack11::MsgPack& message)
		{
			auto columns = this->getAllColumns();
			for (const auto& column : columns) {
				column->broadcast(message);
			}
		}

		//----------
		ofxCvGui::PanelPtr
			Installation::getMiniView()
		{
			auto stack = ofxCvGui::Panels::Groups::makeStrip();
			{
				auto colWidth = ofGetWidth() / this->columns.size();
				auto allColumns = this->getAllColumns();
				for (auto column : allColumns) {
					auto colView = column->getMiniView(colWidth);
					stack->add(colView);
					stack->setHeight(colView->getHeight());
				}
			}
			return stack;
		}
	}
}
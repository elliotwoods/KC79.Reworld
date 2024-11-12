#include "pch_App.h"

#include "Installation.h"
#include "../Utils.h"

using namespace msgpack11;

namespace Modules {
	namespace Hardware {
		//----------
		Installation::Installation()
		{
			this->panel = ofxCvGui::Panels::makeWidgets();
			this->miniView = ofxCvGui::Panels::Groups::makeStrip();
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
			// update columns
			for (const auto& column : this->columns) {
				column->update();
			}

			if (this->needsRebuildMiniView) {
				this->rebuildMiniView();
			}
			if (this->needsRebuildPanel) {
				this->rebuildPanel();
			}
		}

		//----------
		void
			Installation::deserialise(const nlohmann::json& json)
		{
			// Get the arrangement
			{
				if (json.contains("arrangement")) {
					Utils::deserialize(json["arrangement"], this->parameters.arrangement.columns);
					Utils::deserialize(json["arrangement"], this->parameters.arrangement.rows);
					Utils::deserialize(json["arrangement"], this->parameters.arrangement.columnWidth);
					Utils::deserialize(json["arrangement"], this->parameters.arrangement.flipped);
				}
			}

			this->rebuildColumns();

			// Deserialise settings for the columns themselves
			if (json.contains("columns")) {
				auto jsonColumns = json["columns"];

				const auto jsonColumnSettings = json.contains("columnCommonSettings")
					? json["columnCommonSettings"]
					: nlohmann::json();

				auto count = min(this->columns.size(), jsonColumns.size());

				for (size_t i = 0; i < count; i++) {
					auto jsonColumn = jsonColumns[i];
					auto column = this->columns[i];

					// Merge default settings with column settings
					jsonColumn.update(jsonColumnSettings);
					column->deserialise(jsonColumn);
				}
			}

			ofxCvGui::refreshInspector(this);
		}

		//----------
		void
			Installation::rebuildColumns()
		{
			if (!ofxCvGui::isBeingInspected(this)) {
				// In case we are selecting something inside a column
				ofxCvGui::inspect(nullptr);
			}

			this->columns.clear();

			Column::Settings columnSettings;
			{
				columnSettings.countX = this->parameters.arrangement.columnWidth;
				columnSettings.countY = this->parameters.arrangement.rows;
				columnSettings.flipped = this->parameters.arrangement.flipped;
			};

			for (int colIndex = 0; colIndex < this->parameters.arrangement.columns; colIndex++) {
				columnSettings.index = colIndex;
				auto column = make_shared<Column>(columnSettings);
				this->columns.push_back(column);
				column->init();
			}

			this->needsRebuildMiniView = true;
			this->needsRebuildPanel = true;
		}

		//----------
		void
			Installation::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;

			inspector->addParameterGroup(this->parameters);
			inspector->addButton("Rebuild columns", [this]() {
				this->rebuildColumns();
				});
		}

		//----------
		vector<shared_ptr<Column>>
			Installation::getAllColumns() const
		{
			return this->columns;
		}

		//----------
		shared_ptr<Column>
			Installation::getColumnByID(size_t columnID) const
		{
			if (columnID > this->columns.size()) {
				return shared_ptr<Column>();
			}
			return this->columns[columnID];
		}

		//----------
		vector<shared_ptr<Portal>>
			Installation::getAllPortals() const
		{
			vector<shared_ptr<Portal>> portals;
			for (const auto& column : this->columns) {
				auto columnPortals = column->getAllPortals();
				portals.insert(portals.end(), columnPortals.begin(), columnPortals.end());
			}

			return portals;
		}

		//----------
		shared_ptr<Portal>
			Installation::getPortalByTargetID(size_t columnID, Portal::Target target) const
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
			if (allColumns.empty()) {
				return { 0, 0 };
			}

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
			for (const auto& column : this->columns) {
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
			Installation::getPanel()
		{
			return this->panel;
		}

		//----------
		ofxCvGui::PanelPtr
			Installation::getMiniView()
		{
			return this->miniView;
		}

		//----------
		void
			Installation::rebuildMiniView()
		{
			this->miniView->clear();

			if (!this->columns.empty()) {
				auto colWidth = ofGetWidth() / this->columns.size();
				auto allColumns = this->getAllColumns();
				for (auto column : allColumns) {
					auto colView = column->getMiniView(colWidth);
					this->miniView->add(colView);
					this->miniView->setHeight(colView->getHeight());
				}
			}

			this->needsRebuildMiniView = false;
		}

		//----------
		void
			Installation::rebuildPanel() 
		{
			this->panel->clear();

			// Actions (we don't support hot keys for the Installation broadcast actions)
			{
				this->panel->addTitle("Broadcast actions");

				auto buttonStack = this->panel->addHorizontalStack();
				// Special action for poll
				buttonStack->addButton("Poll", [this]() {
					this->pollAll();
					})->setDrawGlyph(u8"\uf059");

					// Add actions
					auto actions = Portal::getActions();
					for (const auto& action : actions) {
						// Max 6 elements per stack then start a new one
						if (buttonStack->getElements().size() >= 6) {
							buttonStack = this->panel->addHorizontalStack();
						}

						auto buttonAction = [this, action]() {
							this->broadcast(action.message);
							};

						auto button = buttonStack->addButton(action.caption, buttonAction);
						button->setDrawGlyph(action.icon);
					}
			}

			this->panel->addSpacer();

			// Add columns
			{
				this->panel->addTitle("Columns");

				auto stack = this->panel->addHorizontalStack();
				float colHeight = 10.0f;
				for (const auto& column : this->columns) {
					auto columnWeak = weak_ptr<Column>(column);
					auto button = make_shared<ofxCvGui::Widgets::SubMenuInspectable>("", column);

					// Custom draw
					{
						button->onDraw += [columnWeak](ofxCvGui::DrawArguments& args) {
							auto column = columnWeak.lock();
							if (ofxCvGui::isBeingInspected(column)) {
								ofPushStyle();
								{
									ofNoFill();
									ofDrawRectangle(args.localBounds);
								}
								ofPopStyle();
							}
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

			this->needsRebuildPanel = false;
		}
	}
}
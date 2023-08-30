#pragma once

#include "Base.h"
#include "crow/crow.h"

#include "Column.h"

namespace Modules {
	class App : public Base
	{
	public:
		App();
		~App();
		
		string getTypeName() const override;
		void init() override;
		void update() override;
		void populateInspector(ofxCvGui::InspectArguments& args);

		shared_ptr<Column> getColumnByID(int) const;

		void dragEvent(const ofDragInfo&);
	protected:
		void setupCrowRoutes();
		crow::SimpleApp crow;
		std::future<void> crowRun;

		map<int, shared_ptr<Column>> columns;
	};
}
#include "Base.h"

namespace Modules {
	class App;

	class RS485 : public Base {
	public:
		RS485(App *);
		void setup() override;
		void update() override;
	protected:
		App * app;
		void processIncoming();
	};
}
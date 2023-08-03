#pragma once

namespace Modules {
	class Base {
	public:
		virtual void setup() {};
		virtual void update() {};
		
		virtual bool processIncoming(Stream &) {
			return false;
		}
	};
}
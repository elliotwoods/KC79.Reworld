#include "pch_App.h"

namespace Utils{
	//----------
	std::string
		getAxisLetter(int axisIndex)
	{
		switch (axisIndex) {
		case 0:
			return "A";
		case 1:
			return "B";
		default:
			ofLogError("Invalid axis index");
			return "";
		}
	}
}
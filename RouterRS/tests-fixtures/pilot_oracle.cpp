// Golden-vector generator for the Pilot kinematics.
//
// The math below is copied VERBATIM from
// Router/src/Modules/Hardware/PerPortal/Pilot.cpp (including implicit
// float/double promotions via the TWO_PI double literal and the
// findClosestAxesCycle current[0] quirk), with parameters passed as
// arguments instead of member lookups.
//
// Build & run (x64 Native Tools prompt):
//   cl /O2 /EHsc /std:c++17 pilot_oracle.cpp && pilot_oracle.exe > pilot-vectors.csv
//
// Output: CSV, one row per evaluation. Floats are printed as IEEE-754 bit
// patterns (%08X) so the Rust test can compare exactly.

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <limits>

// --- openFrameworks definitions (ofConstants.h / ofMath.cpp) ---
#define TWO_PI 6.28318530717958647693
typedef int32_t Steps;

static float ofMap(float value, float inputMin, float inputMax, float outputMin, float outputMax) {
	if (std::fabs(inputMin - inputMax) < std::numeric_limits<float>::epsilon()) {
		return outputMin;
	} else {
		float outVal = ((value - inputMin) / (inputMax - inputMin) * (outputMax - outputMin) + outputMin);
		return outVal;
	}
}

struct vec2 { float x, y; float operator[](int i) const { return i == 0 ? x : y; } };

// --- Pilot.cpp function bodies, verbatim ---

static vec2 positionToPolar(const vec2& position) {
	auto r = std::sqrt(position.x * position.x + position.y * position.y); // glm::length
	auto theta = atan2(position.y, position.x);
	return { r, theta };
}

static vec2 polarToPosition(const vec2& polar) {
	const auto& r = polar.x;
	const auto& theta = polar.y;
	return {
		r * cos(theta)
		, r * sin(theta)
	};
}

static vec2 polarToAxes(const vec2& polar, float offset) {
	// Special case for see-through
	if (polar.x == 0) {
		return {
			0.5, 0.0
		};
	};

	const auto& r = polar.x;
	const auto& theta = polar.y;

	// axes norm coordinates are offset by half rotation from polar
	const auto thetaNorm = theta / TWO_PI - 0.5f;

	// our special sauce for our lenses
	return {
		(float)(thetaNorm - (1 - r) * 0.25 + 0.5 - offset)
		, (float)(thetaNorm + (1 - r) * 0.25 + 0.5 + offset)
	};
}

static vec2 axesToPolar(const vec2& axes, float offset) {
	auto a = axes.x + offset;
	auto b = axes.y - offset;

	auto flattenCycle = [](float x) {
		x = fmodf(x, 1);
		if (x < 0) {
			x += 1;
		}
		return x;
	};

	a = flattenCycle(a);
	b = flattenCycle(b);

	auto r = 2 * a - 2 * b + 1;
	auto thetaNorm = (a + b - 1) / 2;

	if (r > 1.0f) {
		r = 1 - (r - 1);
	}
	if (r < 0.0f) {
		thetaNorm += 0.5f;
		r = -r;
	}

	auto theta = (thetaNorm + 0.5f) * TWO_PI;

	return {
		r
		, (float)theta
	};
}

static vec2 findClosestAxesCycle(const vec2& target, const vec2& current) {
	vec2 adjusted;
	// BUG-COMPAT: both components use current[0], as in Pilot.cpp:839-840
	adjusted.x = target.x + std::round(current.x - target.x);
	adjusted.y = target.y + std::round(current.x - target.y);
	return adjusted;
}

static Steps axisToSteps(float axisValue, int axisIndex, int microstepsPerPrismRotation) {
	float invert = 1.0f;
	if (axisIndex == 1) {
		invert = -1.0f;
	}
	return ofMap(axisValue, 0, 1, 0, invert * microstepsPerPrismRotation);
}

static float stepsToAxis(Steps stepsValue, int axisIndex, int microstepsPerPrismRotation) {
	float invert = 1.0f;
	if (axisIndex == 1) {
		invert = -1.0f;
	}
	return ofMap(stepsValue, 0, invert * microstepsPerPrismRotation, 0, 1);
}

// --- vector generation ---

static uint32_t f2h(float f) { uint32_t u; std::memcpy(&u, &f, 4); return u; }

// deterministic LCG so the table is stable
static uint32_t lcgState = 0x12345678u;
static float lcg(float lo, float hi) {
	lcgState = lcgState * 1664525u + 1013904223u;
	return lo + (hi - lo) * (float)(lcgState >> 8) / (float)0x00FFFFFF;
}

int main() {
	const int MICRO = 189696;
	std::printf("func,in1,in2,in3,out1,out2\n");

	const float rGrid[] = { 0.0f, 1e-6f, 0.001f, 0.1f, 0.25f, 0.5f, 0.70710678f, 0.999f, 1.0f };
	const float thetaGrid[] = { -3.14159274f, -3.0f, -2.0f, -1.5707964f, -1.0f, -0.5f, 0.0f,
	                            0.5f, 1.0f, 1.5707964f, 2.0f, 3.0f, 3.14159274f };
	const float offsetGrid[] = { 0.0f, -0.25f, -0.1f, 0.05f, 0.1f, 0.25f };
	const float axGrid[] = { -2.5f, -1.0f, -0.75f, -0.5f, -0.25f, 0.0f, 0.1f, 0.25f, 0.333333f,
	                         0.5f, 0.75f, 0.999f, 1.0f, 1.5f, 2.0f, 3.25f };

	for (float r : rGrid)
		for (float t : thetaGrid)
			for (float o : offsetGrid) {
				vec2 out = polarToAxes({ r, t }, o);
				std::printf("polarToAxes,%08X,%08X,%08X,%08X,%08X\n", f2h(r), f2h(t), f2h(o), f2h(out.x), f2h(out.y));
			}

	for (float a : axGrid)
		for (float b : axGrid)
			for (float o : offsetGrid) {
				vec2 out = axesToPolar({ a, b }, o);
				std::printf("axesToPolar,%08X,%08X,%08X,%08X,%08X\n", f2h(a), f2h(b), f2h(o), f2h(out.x), f2h(out.y));
			}

	for (float x : axGrid)
		for (float y : axGrid) {
			vec2 out = positionToPolar({ x, y });
			std::printf("positionToPolar,%08X,%08X,,%08X,%08X\n", f2h(x), f2h(y), f2h(out.x), f2h(out.y));
		}

	for (float r : rGrid)
		for (float t : thetaGrid) {
			vec2 out = polarToPosition({ r, t });
			std::printf("polarToPosition,%08X,%08X,,%08X,%08X\n", f2h(r), f2h(t), f2h(out.x), f2h(out.y));
		}

	for (float tgt : axGrid)
		for (float cur : axGrid) {
			vec2 out = findClosestAxesCycle({ tgt, tgt * 0.5f }, { cur, cur * 0.25f });
			std::printf("findClosestAxesCycle,%08X,%08X,%08X,%08X,%08X\n",
				f2h(tgt), f2h(tgt * 0.5f), f2h(cur), f2h(out.x), f2h(out.y));
		}

	for (float v : axGrid)
		for (int idx = 0; idx < 2; idx++) {
			Steps s = axisToSteps(v, idx, MICRO);
			std::printf("axisToSteps,%08X,%d,%d,%d,\n", f2h(v), idx, MICRO, (int)s);
			float back = stepsToAxis(s, idx, MICRO);
			std::printf("stepsToAxis,%d,%d,%d,%08X,\n", (int)s, idx, MICRO, f2h(back));
		}

	// random sweeps for breadth
	for (int i = 0; i < 2000; i++) {
		float r = lcg(-0.5f, 1.5f), t = lcg(-7.0f, 7.0f), o = lcg(-0.25f, 0.25f);
		vec2 out = polarToAxes({ r, t }, o);
		std::printf("polarToAxes,%08X,%08X,%08X,%08X,%08X\n", f2h(r), f2h(t), f2h(o), f2h(out.x), f2h(out.y));
		float a = lcg(-4.0f, 4.0f), b = lcg(-4.0f, 4.0f);
		vec2 out2 = axesToPolar({ a, b }, o);
		std::printf("axesToPolar,%08X,%08X,%08X,%08X,%08X\n", f2h(a), f2h(b), f2h(o), f2h(out2.x), f2h(out2.y));
	}
	return 0;
}

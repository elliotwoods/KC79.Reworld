#pragma once
#include "Base.h"
#include "Exception.h"

namespace Modules {
	class App;

	class RS485 : public Base {
	public:
		RS485(App *);
		const char * getTypeName() const;
		void setup() override;
		void update() override;
		
		void sendStatusReport();
		void sendPositions();

		// Use this function if you want to manually send an ACK
		// e.g. if the message starts a routine which takes time (init/home/etc)
		static void sendACKEarly(bool success);

		// Use this to set disableACK to true for this receive frame
		// e.g. for when you manually give a response
		// Returns false if already no ACK was required
		static void noACKRequired();

		// Are you allowed to reply to a message?
		static bool replyAllowed();

		// Verifies the trailing [seq, crc16] this frame appended against everything read so
		// far (see msgpack::COBSRWStream::checkChecksum). Call this as the LAST thing a
		// handler does before any side effect it's about to commit -- e.g. right before
		// setTargetPosition, right before starting a routine, right before the "FW" handler's
		// NVIC_SystemReset() -- not at the start of a message. See Protocol.md /
		// protocol-hardening.md for why this is shaped as a commit-time check rather than a
		// whole-packet gate. On success, the sender's seq is captured for the next outgoing
		// frame's echo (see finishFrame()); on failure (mismatch, malformed trailer, or the
		// trailer not arriving within a few ms) returns false and nothing is captured --
		// treat exactly like a CRC failure, i.e. do not commit.
		//
		// Returns true unconditionally (no read attempted) while verifyChecksumEnabled is
		// false -- the default. This firmware always SENDS the trailer (see finishFrame()),
		// but a Router that hasn't landed its own Stage 2 (appending seq+CRC to what it
		// sends) never puts one on the wire, and requiring one from every command would break
		// remote updates against every Router in the field today. Flip verifyChecksumEnabled
		// on (the "verifyChecksum" msgpack key, mirroring debugLightsEnabled) once the whole
		// fleet + Router are confirmed to be sending it -- matching the staged rollout table
		// in protocol-hardening.md (append: on by default; verify: off by default).
		static bool checkChecksum();

		static void setVerifyChecksumEnabled(bool);
		static bool getVerifyChecksumEnabled();

		bool hasAnySignalBeenReceived() const;
	protected:
		App * app;
		static RS485 * instance;

		void processIncoming();
		Exception processCOBSPacket(bool & isForUs);

		void beginTransmission();
		void endTransmission();

		// Appends [seq, crc16] (seq = the last verified request's seq, echoed) and ends the
		// transmission. Every sender uses this instead of calling endTransmission() directly,
		// so the trailer can never be forgotten at one call site among several.
		void finishFrame();

		void sendACK(bool success);

		bool disableACK = false;
		bool sentACKEarly = false;

		bool anySignalReceived = false;

		// The most recently verified (checkChecksum()-passed) request's seq, echoed in every
		// outgoing frame's trailer. 0 until the first successful verification, or forever if
		// the Router hasn't started sending seq numbers yet (Stage 3) -- either way this stays
		// backward compatible, since old Router code ignores trailing array elements.
		uint8_t lastRxSeq = 0;

		bool verifyChecksumEnabled = false;
	};
}
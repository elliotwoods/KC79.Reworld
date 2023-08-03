#include <stdint.h>

// Note we only use a U7 for this data
enum class MessageType : uint8_t
{
	ServerBroadcast = 0x00,
	ServerUnicast = 0x01,
	ClientReply = 0x11
};
#%% imports
from cobs import cobs
import msgpack

#%% encode
# Encoded message in hexadecimal
encoded_message_hex = "03 93 D0 08 D0 01 81 A1 70 94 D2 05 01 72 80 D2 01 01 01 02 D2 05 01 72 80 D2 01 01 01 01 00"
encoded_message = bytes.fromhex(encoded_message_hex.replace(" ", ""))

# Remove trailing 0x00
encoded_message = encoded_message.rstrip(b"\x00")

# Print message length
print("Encoded message length:", len(encoded_message))

# Decode with COBS
try:
	cobs_decoded = cobs.decode(encoded_message)
	print("COBS Decoded:", cobs_decoded)
except cobs.DecodeError as e:
	print("COBS decoding failed:", e)
	cobs_decoded = None

	# Manually decode the COBS and print the position in the string where the error occurred
	# This is useful for debugging the issue
	for i in range(len(encoded_message), 1, -1):
		try:
			cobs_decoded = cobs.decode(encoded_message[:i])
			print("COBS decoding failed after position", i)
			break
		except cobs.DecodeError as e:
			continue

# Decode with msgpack
if cobs_decoded:
	try:
		decoded_message = msgpack.unpackb(cobs_decoded)
		print("MessagePack Decoded:", decoded_message)
	except msgpack.exceptions.UnpackException as e:
		print("MessagePack decoding failed:", e)

# %%

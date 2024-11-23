#%% imports
from cobs import cobs
import msgpack

#%% encode
# Encoded message in hexadecimal
encoded_message_hex = "03 93 D0 05 D0 01 84 DB 01 01 07 03 61 70 70 82 DB 01 01 09 06 75 70 54 69 6D 65 CE 05 12 3A 93 DB 01 01 0A 07 76 65 72 73 69 6F 6E DB 01 01 1B 18 50 6F 72 74 61 6C 20 76 32 30 32 34 2D 31 30 2D 30 32 5F 31 31 2E 34 31 DB 01 01 07 03 6D 63 61 86 DB 01 01 0B 08 70 6F 73 69 74 69 6F 6E D2 01 01 01 02 DB 01 01 11 0E 74 61 72 67 65 74 50 6F 73 69 74 69 6F 6E D2 01 01 01 02 DB 01 01 10 0C 68 65 61 6C 74 68 53 74 61 74 75 73 84 DB 01 01 12 0E 6D 65 61 73 75 72 65 43 79 63 6C 65 4F 4B C3 DB 01 01 0E 0A 53 77 69 74 63 68 65 73 4F 4B C3 DB 01 01 0E 0A 62 61 63 6B 6C 61 73 68 4F 4B C3 DB 01 01 0A 06 68 6F 6D 65 4F 4B C3 DB 01 01 0F 0C 6D 61 78 69 6D 75 6D 53 70 65 65 64 D2 01 02 37 02 DB 01 01 0F 0C 61 63 63 65 6C 65 72 61 74 69 6F 6E D2 01 04 27 10 DB 01 01 0F 0C 6D 69 6E 69 6D 75 6D 53 70 65 65 64 D2 01 01 03 05 DB 01 01 07 03 6D 63 62 86 DB 01 01 0B 08 70 6F 73 69 74 69 6F 6E D2 01 01 01 02 DB 01 01 11 0E 74 61 72 67 65 74 50 6F 73 69 74 69 6F 6E D2 01 01 01 02 DB 01 01 10 0C 68 65 61 6C 74 68 53 74 61 74 75 73 84 DB 01 01 12 0E 6D 65 61 73 75 72 65 43 79 63 6C 65 4F 4B C3 DB 01 01 0E 0A 53 77 69 74 63 68 65 73 4F 4B C3 DB 01 01 0E 0A 62 61 63 6B 6C 61 73 68 4F 4B C3 DB 01 01 0A 06 68 6F 6D 65 4F 4B C3 DB 01 01 0F 0C 6D 61 78 69 6D 75 6D 53 70 65 65 64 D2 01 02 37 02 DB 01 01 0F 0C FC"
encoded_message = bytes.fromhex(encoded_message_hex.replace(" ", ""))

# Remove trailing 0x00
encoded_message = encoded_message.rstrip(b"\x00")

# Decode with COBS
try:
	cobs_decoded = cobs.decode(encoded_message)
	print("COBS Decoded:", cobs_decoded)
except cobs.DecodeError as e:
	print("COBS decoding failed:", e)
	cobs_decoded = None

# Decode with msgpack
if cobs_decoded:
	try:
		decoded_message = msgpack.unpackb(cobs_decoded)
		print("MessagePack Decoded:", decoded_message)
	except msgpack.exceptions.UnpackException as e:
		print("MessagePack decoding failed:", e)

# %%

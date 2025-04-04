import re

# The circuitpy thing gives MS-bit first instead
# of LS-bit first decoding so we have to flip them
# around.

def rev(b):
    b = (b & 0xF0) >> 4 | (b & 0x0F) << 4
    b = (b & 0xCC) >> 2 | (b & 0x33) << 2
    b = (b & 0xAA) >> 1 | (b & 0x55) << 1
    return b


while True:
    try:
        line = input()
        if m := re.match(r"\((.*)\)(.*)", line):
            paren = m.group(1)
            bs = [rev(int(s.strip())) for s in paren.split(",")]
            v = bs[0] | bs[1] << 8 | bs[2] << 16 | bs[3] << 24
            print(f"{v:x}", m.group(2))
    except EOFError:
        break

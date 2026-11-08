#!/usr/bin/env python3
"""Raw hidraw probe for the SteelSeries Apex 3 lighting protocol.

Usage (run with `python3 -I`):
  probe_apex3.py all RRGGBB
  probe_apex3.py zone N RRGGBB
  probe_apex3.py brightness N
  probe_apex3.py walk
  probe_apex3.py raw HEX...        # send arbitrary bytes, padded to 33
"""
import glob
import os
import sys

REPORT_LEN = 33  # 0x00 (no report IDs) + 32-byte payload
ZONES = 10


def find_node():
    for uevent in glob.glob("/sys/class/hidraw/hidraw*/device/uevent"):
        text = open(uevent).read()
        if "HID_ID=0003:00001038:0000161A" in text and any(
            line.startswith("HID_PHYS=") and line.endswith("input3")
            for line in text.splitlines()
        ):
            return "/dev/" + uevent.split("/")[4]
    sys.exit("Apex 3 (1038:161a) interface 3 not found")


def send(payload):
    report = bytes(payload).ljust(REPORT_LEN, b"\x00")[:REPORT_LEN]
    fd = os.open(find_node(), os.O_WRONLY)
    try:
        os.write(fd, report)
    finally:
        os.close(fd)
    print(report.hex(" "))


def rgb(hexstr):
    h = hexstr.lstrip("#")
    return [int(h[i : i + 2], 16) for i in (0, 2, 4)]


def colors(zone_colors):
    payload = [0x00, 0x0B, 0x00]
    for c in zone_colors:
        payload += c
    send(payload)


def main(argv):
    if not argv:
        sys.exit(__doc__)
    cmd = argv[0]
    if cmd == "all":
        colors([rgb(argv[1])] * ZONES)
    elif cmd == "zone":
        n = int(argv[1])
        colors([rgb(argv[2]) if i == n else [0, 0, 0] for i in range(ZONES)])
    elif cmd == "brightness":
        send([0x00, 0x0A, 0x00, int(argv[1])])
    elif cmd == "walk":
        for n in range(ZONES):
            colors([[255, 255, 255] if i == n else [0, 0, 0] for i in range(ZONES)])
            input(f"zone {n} lit - note which area, then Enter ")
    elif cmd == "raw":
        send([int(b, 16) for b in argv[1:]])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv[1:])

# Apex 3 (1038:161a) lighting protocol — verified on hardware 2026-11-08

Device: USB interface 3 (vendor usage page 0xFFC0). Its report descriptor has
**no report IDs**, so every hidraw write is 33 bytes: `0x00` + 32-byte payload.

| Command | Bytes (33, zero-padded) | Verified |
|---|---|---|
| Set all 10 zone colours | `00 0B 00` + `R G B` × 10 | yes — whole board changed colour |
| Brightness 0–100 | `00 0A 00 <level>` | yes — 30 dim, 100 bright |
| save_to_flash | `00 09 00` | not sent (never send casually) |

No init / driver-mode frame is needed: colours take effect immediately, and
there is no APPLY step. This matches upstream steelseriesgg-rs @84a0152.

## Zone layout

The 10 zones are vertical bands across the full board, left to right:

| Index | Position |
|---|---|
| 0 | leftmost band |
| 1–8 | successive bands, left → right |
| 9 | rightmost band (right edge of numpad) |

Method: zones 0–9 set to red, orange, yellow, green, cyan, blue, purple, pink,
white, off — seen in order left → right; zone 9 alone lit the far right strip.
Exact key-to-band boundaries can't be seen: the zone LEDs bleed across keys,
so with alternating colours there are no clean edges (checked 2026-11-08). The
layout file (`assets/layouts/apex3-ansi.toml`) therefore assigns zones by key
centre: `zone = floor(centre_x / 22.5 * 10)`, i.e. 10 equal-width bands over
the full 22.5-unit board.

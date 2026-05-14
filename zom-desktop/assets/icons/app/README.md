# zom · App Icon export

Locked design: phosphor green `>_` + 3-row staggered keyboard, on a deep ink (#0E1116) background.

## Files

- `zom-icon.svg` — vector master, includes iOS squircle background
- `zom-icon-square.svg` — vector master, square corners (for stores that mask)
- `rounded/zom-{size}.png` — pre-masked, ready to drop into web / desktop / Android
- `square/zom-{size}.png` — square corners, use for App Store Connect 1024px upload

## Sizes included

1024, 512, 256, 192, 180, 167, 152, 128, 120, 87, 80, 60, 48, 32, 16

## Palette

- Background: `#0E1116`
- `>_` glyph: `#5BE584` (phosphor)
- Keyboard keys: `#D9CFB8` (bone)
- Spacebar accent: `#5BE584` (phosphor)

The SVG is the source of truth — re-export at any size by editing it and re-running the rasterizer.

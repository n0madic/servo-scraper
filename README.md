# servo-scraper

A minimal headless web scraper built on the [Servo](https://servo.org/) browser engine. Captures screenshots and/or HTML content from web pages with full JavaScript execution.

## Features

- Full-page or viewport-only screenshots (PNG, JPG, BMP)
- HTML capture via JS evaluation (`document.documentElement.outerHTML`)
- Configurable viewport size, load timeout, and post-load JS settle time
- Software rendering — no GPU or display server required
- ~55MB stripped binary

## Prerequisites

- Rust 1.86+
- System dependencies required by Servo (see [Servo build instructions](https://book.servo.org/building/building.html))

## Build

```bash
git clone --recurse-submodules https://github.com/n0madic/servo-scraper.git
cd servo-scraper
make
```

Binary: `target/release/servo-scraper`

## Usage

```bash
# Screenshot
servo-scraper --screenshot page.png https://example.com

# Full-page screenshot
servo-scraper --fullpage --screenshot page.png https://example.com

# HTML capture
servo-scraper --html page.html https://example.com

# Both + custom viewport
servo-scraper --screenshot page.png --html page.html --width 1920 --height 1080 https://example.com
```

### Options

| Option | Description | Default |
|---|---|---|
| `--screenshot <PATH>` | Save screenshot (png, jpg, bmp) | — |
| `--html <PATH>` | Save page HTML | — |
| `--fullpage` | Capture full scrollable page | off |
| `--width <PX>` | Viewport width | 1280 |
| `--height <PX>` | Viewport height | 720 |
| `--timeout <SEC>` | Max page load wait | 30 |
| `--wait <SEC>` | Post-load JS settle time | 2.0 |

## Updating Servo

```bash
make update-servo
git commit -m "Update servo submodule"
```

## License

MPL-2.0

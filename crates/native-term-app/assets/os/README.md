# The systems' pictures

What a host's row shows in place of the picture any host has, once its
server said what system it is of (`src/server.rs`, `src/logos.rs`).
Each is 96 x 96 pixels, the logo in the middle, written by
`tools/render-os-logos.py` from the sources below.

| Picture | From | File there | As it is |
| --- | --- | --- | --- |
| `arch.png` | gilbarbara/logos | `logos/archlinux.svg` | its own colours |
| `centos.png` | gilbarbara/logos | `logos/centos-icon.svg` | its own colours |
| `debian.png` | gilbarbara/logos | `logos/debian.svg` | its own colours |
| `fedora.png` | gilbarbara/logos | `logos/fedora.svg` | its own colours |
| `freebsd.png` | gilbarbara/logos | `logos/freebsd.svg` | its own colours |
| `linux.png` | gilbarbara/logos | `logos/linux-tux.svg` | its own colours |
| `linuxmint.png` | gilbarbara/logos | `logos/linux-mint.svg` | its own colours |
| `manjaro.png` | gilbarbara/logos | `logos/manjaro.svg` | its own colours |
| `raspbian.png` | gilbarbara/logos | `logos/raspberry-pi.svg` | its own colours |
| `rhel.png` | gilbarbara/logos | `logos/redhat-icon.svg` | its own colours |
| `rocky.png` | gilbarbara/logos | `logos/rocky-linux-icon.svg` | its own colours |
| `ubuntu.png` | gilbarbara/logos | `logos/ubuntu.svg` | its own colours |
| `windows.png` | gilbarbara/logos | `logos/microsoft-windows-icon.svg` | its own colours |
| `zorin.png` | gilbarbara/logos | `logos/zorin-os.svg` | its own colours |
| `almalinux.png` | lukas-w/font-logos | `vectors/almalinux.svg` | a shape, white: coloured when drawn |
| `alpine.png` | lukas-w/font-logos | `vectors/alpine.svg` | a shape, white: coloured when drawn |
| `deepin.png` | lukas-w/font-logos | `vectors/deepin.svg` | a shape, white: coloured when drawn |
| `elementary.png` | lukas-w/font-logos | `vectors/elementary.svg` | a shape, white: coloured when drawn |
| `endeavouros.png` | lukas-w/font-logos | `vectors/endeavour.svg` | a shape, white: coloured when drawn |
| `kali.png` | lukas-w/font-logos | `vectors/kali-linux.svg` | a shape, white: coloured when drawn |
| `macos.png` | lukas-w/font-logos | `vectors/apple.svg` | a shape, white: coloured when drawn |
| `openbsd.png` | lukas-w/font-logos | `vectors/openbsd.svg` | a shape, white: coloured when drawn |
| `suse.png` | lukas-w/font-logos | `vectors/opensuse.svg` | a shape, white: coloured when drawn |

A shape is taken where the other source has no logo of the system, or has
one that does not read 16 pixels high or on a dark row (SUSE's with its
name beside it, Apple's and elementary's in black).

- **gilbarbara/logos** (https://github.com/gilbarbara/logos), at
  `37a6b807fd71c622efea27a9309b5d4edc792969` (2026-09-27): CC0 1.0
  Universal. The logos as their owners publish them, in colour.
- **lukas-w/font-logos** (https://github.com/lukas-w/font-logos), at
  `d3bf5d299e54595db1b19681a0cc57ab10454857` (v1.4.0, 2026-07-20): the
  Unlicense. Shapes in one colour, for the systems the other has none
  of.

Both give up the copyright in their files. The logos stay the marks of
their owners, and are used here to say what system a server runs, which
is what they are for.

The systems are those a server can say it is of (see `src/server.rs`
for what servers say) and those the person may say a host runs
(`NativeTermSystem`, chosen in the host's dialog). To add one: its line in
`LOGOS` of `tools/render-os-logos.py`, the script run (it needs inkscape
and Pillow), and the system in `server::Os` and `logos::kept`; the test
`every_system_has_its_picture` says whether the picture is as it has
to be.

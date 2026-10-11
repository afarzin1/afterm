# Terminal text colours

Keep the ground, the default ink, the cursor, and the selection. Those already match the chrome. Replace the 16 ANSI colours. Those are Terminus VGA, and they are what you look at all day.

The job of the 16 is semantic, not decorative: red fails, yellow warns, green is fine, blue recedes, dim is comments. Neighbouring slots must be distinguishable at a glance. Each bright is the same hue, lighter — not a second, louder colour.

## Leave these alone

| Slot | Stockholm | Why |
|---|---|---|
| background | `#1a202c` | Chrome `--bg`. The terminal is the window. |
| foreground | `#e6ebf2` | Chrome `--text`. Default ink, not ANSI white. |
| cursor / cursorAccent | `#e6ebf2` / `#1a202c` | A hole in the text. |
| selection | `#33405a` | Already designed. |
| input band | `#414c60` | Already designed. |

## Stockholm ANSI

Snap yellow, dim, and the two reds to tokens you already have. Take the rest from palettes that are proven on this kind of ground (One Dark, Tokyo Night), not from Terminus.

| Slot | Now | Use | Role |
|---|---|---|---|
| black | `#000000` | `#12161e` | Darker than the ground, not a punch-out. |
| red | `#cd0000` | `#e06c75` | Error, `git` delete. |
| green | `#00cd00` | `#98c379` | Ok, `git` add, most prompts. Yellow-green so it does not collide with the chrome accent or with cyan. |
| yellow | `#cdcd00` | `#d8b070` | Warn, `git` modify. This is `--warn`. |
| blue | `#3b5bdb` | `#7aa2f7` | Directories, paths. Lighter than now so it reads *on* a blue ground instead of vibrating against it. |
| magenta | `#cd00cd` | `#c678dd` | Branches, rare. |
| cyan | `#00cdcd` | `#56b6c2` | Usernames, types, links. Kills the VGA teal. |
| white | `#e5e5e5` | `#c8d2e0` | ANSI white, one step below default ink (`--text-2`), so bright white still has somewhere to go. |
| brightBlack | `#7f7f7f` | `#8e9bb0` | Comments, dim. Tinted (`--muted`), not a neutral grey that goes brown on this ground. |
| brightRed | `#ff0000` | `#ff5a5a` | `--danger`. |
| brightGreen | `#00ff00` | `#b8d99a` | Same green, lighter. |
| brightYellow | `#ffff00` | `#e8c98a` | Same yellow, lighter. |
| brightBlue | `#7b8cff` | `#9db6ff` | Same blue, lighter. |
| brightMagenta | `#ff00ff` | `#d7a0ea` | Same magenta, lighter. |
| brightCyan | `#00ffff` | `#7dd0db` | Same cyan, lighter. |
| brightWhite | `#ffffff` | `#ffffff` | The cap. Rare; default text stays `--text`. |

Do not reuse `#4cbf7c` as ANSI green. That is a tab underline and a primary button. A prompt that matches the chrome accent looks selected.

## Carbon

Same 16 hues, chroma pulled toward the cool-grey axis — not zero. Greyscale ANSI was the mistake: `ls` and `git` have nothing else to speak with. Ground, ink, and selection stay Carbon’s. Chrome can stay grey; the host’s output cannot.

## Judge it with three screens, not a swatch row

`ls --color`, `git status`, and the preview `colors` command. Directories must recede, errors must not, comments must be quieter than body text, and no slot should glow.

Do not add a theme gallery. This is Stockholm’s default, and Carbon’s, done properly.

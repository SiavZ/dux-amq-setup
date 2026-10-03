---
title: Managing Themes
description: Switch the built-in themes, preview them live, or write your own from scratch.
group: Guides
order: 40
---

yaran ships with a generous set of built-in themes and lets you write your own.

> [!IMPORTANT]
> Theming is a terminal UI feature. The web UI is dark-only: it ships one tuned dark
> palette, ignores your `theme` setting, and does not follow your system light/dark
> preference. A theme you write changes the terminal UI only.

## Changing the theme

Open the **theme picker** from the terminal UI's command palette and arrow through the
options with a live preview. It discovers every built-in plus anything you have
authored, and labels where each one came from.

![The terminal UI theme picker open over the workspace, listing yaran_dark as the current theme with the bundled themes below it.](/screens/tui-theme-picker.png)

To set it by hand instead, edit the `[ui]` section of `config.toml` and restart:

```toml
[ui]
theme = "yaran_dark"   # the default
```

Built-in names use underscores:

```toml
theme = "catppuccin_mocha"
theme = "nord"
theme = "dracula"
theme = "tokyo_night"
theme = "gruvbox_dark"
```

There are far more: Catppuccin's four flavors, Tokyo Night's variants, Solarized,
Rose Pine, Everforest, Kanagawa, One Dark, and others. The picker is the list that
never goes stale.

## How a theme name is resolved

When you set `theme = "<name>"`, yaran looks in this order:

1. A user theme at `<config dir>/themes/<name>.toml`. Your own themes win first.
2. The bundled `yaran_dark` theme.
3. An [Opaline](https://github.com/hyperb1iss/opaline) built-in (Catppuccin, Nord,
   Dracula, and friends).

> [!NOTE]
> A name that matches nothing is not fatal. yaran tells you so and falls back to a safe
> default rather than launching into an unreadable color scheme.

## Writing your own theme

Custom themes are TOML files. Put one at:

- **Linux:** `~/.config/yaran/themes/<name>.toml`
- **macOS:** `~/.yaran/themes/<name>.toml`

then point your config at it:

```toml
[ui]
theme = "my_theme"   # matches my_theme.toml in the themes directory
```

The file name, without `.toml`, is the theme's id.

> [!TIP]
> Start from the theme yaran already uses. Grab
> [`assets/themes/yaran_dark.toml`](https://github.com/patrickdappollonio/dux/blob/main/assets/themes/yaran_dark.toml)
> from the repository, drop it in your themes directory under a new name, and recolor
> it. It defines every surface yaran paints, so you will never hit a missing color.

### The shape of a theme file

Theme files use the Opaline format. Three parts matter most:

```toml
[meta]
name = "My Theme"
author = "you"
variant = "dark"        # or "light"
description = "A custom yaran theme"

[palette]
# Name your colors once, reuse them everywhere below.
bg     = "#11111d"
fg     = "#f4f7ff"
accent = "#00d4ff"
pink   = "#ff4fd8"

[tokens]
# yaran.* tokens map a color onto a specific yaran UI surface.
"yaran.app_bg"         = "bg"
"yaran.text_fg"        = "fg"
"yaran.border_focused" = "accent"
"yaran.title_focused"  = "accent"
"yaran.selection_bg"   = "pink"
# ...and so on for every surface you want to control.
```

You have two ways to define colors:

- **Set `yaran.*` tokens explicitly** for control over each surface: borders, headers,
  selection, diff colors, status line, and the rest. This is what the bundled theme
  does.
- **Rely on the fallback.** Any `yaran.*` token you leave out is derived from Opaline's
  standard semantic tokens (`text.*`, `bg.*`, `accent.*`, `border.*`, `code.*`). A
  complete, standard Opaline theme works in yaran as-is, even if it was never written
  with yaran in mind.

Names inside dialogs (branches, paths, agent and project names) are drawn as small
chips in your dialog's own colors, swapped: the dialog background (`yaran.overlay_bg`)
on the dialog text color (`yaran.text_fg`). A chip is therefore exactly as readable as
the sentence around it, and there is nothing extra to set. If your theme still sets
`yaran.name_fg` or `yaran.name_bg` from an older version of yaran, you can delete them: yaran
no longer reads either token and ignores them if they are there.

Save the file and open the theme picker: your theme is listed alongside the built-ins,
labeled as user-authored, with the same live preview. Tweak, save, re-pick, repeat.

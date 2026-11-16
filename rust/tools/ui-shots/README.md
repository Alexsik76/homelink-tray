# ui-shots

Renders HomeLink windows with Slint's software renderer, without a display, to PNG files:
the wizard steps 3–5 (top and scrolled to the end), the settings list in both themes, the help
page and the tray panel with reason lines. Made on 16.11.2026 to find why wizard steps could not
be scrolled to their end (a wrapped `Text` in a nested layout was measured as one line unless it
has an explicit width).

```
rust/tools/ui-shots/run.sh /tmp/shots
```

`run.sh` copies `rust/app/ui`, exposes the wizard's scroll position in the copy and copies
`fill_words` from `rust/app/src/manage.rs`, so the texts are those of the app. The scenarios are in
`src/main.rs`.

On a machine without `pkg-config` (agent-claude), give the build a stand-in that answers for
fontconfig and a link path to the system library:

```
mkdir -p /tmp/pc/lib && ln -sf /usr/lib/x86_64-linux-gnu/libfontconfig.so.1 /tmp/pc/lib/libfontconfig.so
printf '#!/bin/sh\ncase "$*" in *--modversion*) echo 2.15.0;; *fontconfig*) echo "-L/tmp/pc/lib -lfontconfig";; *) exit 1;; esac\n' > /tmp/pc/pkg-config && chmod +x /tmp/pc/pkg-config
PATH=/tmp/pc:$PATH RUSTFLAGS="-L /tmp/pc/lib" rust/tools/ui-shots/run.sh /tmp/shots
```

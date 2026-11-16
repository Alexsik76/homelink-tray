#!/bin/sh
# Renders the wizard steps, the settings window and the tray panel to PNG files in $1 (default
# ./shots). Linux only; needs libfontconfig. Without pkg-config, set PKG_CONFIG to a script that
# prints "-L<dir> -lfontconfig" for fontconfig (see README.md).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-$here/shots}
rm -rf "$here/ui" && cp -r "$here/../../app/ui" "$here/ui"
# Expose the wizard's scroll position (probe only; the app itself is unchanged).
sed -i 's|^export component WizardWindow inherits Window {$|&\n    out property <length> dbg-content: flick.content-height;\n    out property <length> dbg-view: flick.height;\n    in property <length> dbg-scroll;|' "$here/ui/wizard.slint"
sed -i '0,/            flick := Flickable {/s||&\n                content-y: -root.dbg-scroll;|' "$here/ui/wizard.slint"
sed -n '/^fn fill_words/,/^}/p' "$here/../../app/src/manage.rs" > "$here/src/words.rs"
mkdir -p "$out"
cd "$here" && cargo run -q -- "$out"

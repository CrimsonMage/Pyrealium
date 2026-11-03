#!/bin/sh
# Linux/macOS launcher for the content editor (world builder). Needs Python 3 with Tk.
# The 3D view uses OpenGL through GLX (X11 or XWayland); PYREALIUM_EDITOR_SOFTWARE=1 forces the software view.
cd "$(dirname "$0")" || exit 1
exec python3 tools/content_editor.py "$@"

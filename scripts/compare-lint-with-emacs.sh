#!/usr/bin/env bash
# compare-lint-with-emacs.sh — Run org-lint and org-tools check, compare diagnostics.
#
# Usage:
#   ./scripts/compare-lint-with-emacs.sh file1.org [file2.org ...]
#
# Requirements:
#   - emacs (with org-mode)
#   - org binary

set -euo pipefail

ORGFMT="${ORGFMT:-cargo run --release -p org --}"
EMACS="${EMACS:-emacs}"

if [[ $# -eq 0 ]]; then
    echo "Usage: $0 <file.org> [file2.org ...]"
    echo ""
    echo "Runs both Emacs org-lint and org-tools check on each file,"
    echo "showing diagnostics side by side."
    exit 1
fi

if ! command -v "$EMACS" &>/dev/null; then
    echo "Error: emacs not found."
    exit 2
fi

# Build org if needed
if [[ "$ORGFMT" == cargo* ]]; then
    cargo build --release 2>/dev/null
    ORGFMT="./target/release/org"
fi

# Emacs lisp for org-lint
ELISP_LINT='
(progn
  (require (quote org))
  (require (quote org-lint) nil t)
  (find-file-literally (nth 0 command-line-args-left))
  (org-mode)
  (let ((results (org-lint)))
    (dolist (r results)
      (let* ((line (aref (cadr r) 0))
             (trust (aref (cadr r) 1))
             (message (aref (cadr r) 2)))
        (princ (format "%s:%s: %s [%s]\n"
                       (nth 0 command-line-args-left)
                       (or line "?")
                       message
                       trust)))))
  (kill-emacs 0))
'

for FILE in "$@"; do
    if [[ ! -f "$FILE" ]]; then
        echo "Warning: $FILE not found, skipping."
        continue
    fi

    BASENAME=$(basename "$FILE")

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "  $BASENAME"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    echo ""
    echo "--- Emacs org-lint ---"
    "$EMACS" --batch --no-init-file \
        --eval "$ELISP_LINT" \
        "$FILE" 2>/dev/null || echo "(org-lint returned no results or failed)"

    echo ""
    echo "--- org-tools check ---"
    $ORGFMT fmt check "$FILE" 2>/dev/null || true

    echo ""
done

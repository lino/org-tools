#!/usr/bin/env bash
# compare-with-emacs.sh — Format org files with both Emacs and org-tools, show differences.
#
# Usage:
#   ./scripts/compare-with-emacs.sh file1.org [file2.org ...]
#   ./scripts/compare-with-emacs.sh tests/fixtures/edge_cases/*.org
#
# Requirements:
#   - emacs (any recent version with org-mode)
#   - org binary (built via `cargo build --release`)
#   - diff, colordiff (optional, for colored output)

set -euo pipefail

ORGFMT="${ORGFMT:-cargo run --release -p org --}"
EMACS="${EMACS:-emacs}"
DIFF_CMD="diff"

# Use colordiff if available
if command -v colordiff &>/dev/null; then
    DIFF_CMD="colordiff"
fi

if [[ $# -eq 0 ]]; then
    echo "Usage: $0 <file.org> [file2.org ...]"
    echo ""
    echo "Formats each file with both Emacs org-mode and org-tools,"
    echo "then shows the differences."
    echo ""
    echo "Environment variables:"
    echo "  ORGFMT  — org command (default: cargo run --release -p org --)"
    echo "  EMACS   — emacs binary (default: emacs)"
    exit 1
fi

# Check that emacs is available
if ! command -v "$EMACS" &>/dev/null; then
    echo "Error: emacs not found. Set EMACS= to point to your emacs binary."
    exit 2
fi

# Build org if using cargo run
if [[ "$ORGFMT" == cargo* ]]; then
    echo "Building org-tools..."
    cargo build --release 2>/dev/null
    ORGFMT="./target/release/org"
fi

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

TOTAL=0
SAME=0
DIFFERENT=0
ERRORS=0

# Emacs lisp script for formatting
ELISP_FORMAT='
(progn
  (require (quote org))
  (find-file (nth 0 command-line-args-left))
  ;; Align all tables
  (goto-char (point-min))
  (while (re-search-forward "^[ \t]*|" nil t)
    (org-table-align))
  ;; Remove trailing whitespace
  (delete-trailing-whitespace)
  ;; Normalize blank lines (collapse 3+ to 2)
  (goto-char (point-min))
  (while (re-search-forward "\n\\{3,\\}" nil t)
    (replace-match "\n\n"))
  ;; Save to output file
  (write-file (nth 1 command-line-args-left))
  (kill-emacs 0))
'

for FILE in "$@"; do
    if [[ ! -f "$FILE" ]]; then
        echo "Warning: $FILE not found, skipping."
        continue
    fi

    TOTAL=$((TOTAL + 1))
    BASENAME=$(basename "$FILE")

    EMACS_OUT="$TMPDIR/emacs-$BASENAME"
    ORGFMT_OUT="$TMPDIR/org-tools-$BASENAME"

    # Format with Emacs
    if ! "$EMACS" --batch --no-init-file \
        --eval "$ELISP_FORMAT" \
        "$FILE" "$EMACS_OUT" 2>/dev/null; then
        echo "[$BASENAME] ERROR: Emacs formatting failed"
        ERRORS=$((ERRORS + 1))
        continue
    fi

    # Format with org-tools
    if ! $ORGFMT fmt --stdout "$FILE" > "$ORGFMT_OUT" 2>/dev/null; then
        echo "[$BASENAME] ERROR: org-tools formatting failed"
        ERRORS=$((ERRORS + 1))
        continue
    fi

    # Compare
    if diff -q "$EMACS_OUT" "$ORGFMT_OUT" > /dev/null 2>&1; then
        echo "[$BASENAME] ✓ identical"
        SAME=$((SAME + 1))
    else
        echo ""
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "[$BASENAME] ✗ DIFFERENCES FOUND"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo ""
        echo "--- Emacs (left)  vs  org-tools (right) ---"
        $DIFF_CMD -u \
            --label "emacs: $BASENAME" "$EMACS_OUT" \
            --label "org-tools: $BASENAME" "$ORGFMT_OUT" \
            || true  # diff exits 1 when files differ
        echo ""
        DIFFERENT=$((DIFFERENT + 1))
    fi
done

echo ""
echo "════════════════════════════════════════════════════"
echo "Summary: $TOTAL files compared"
echo "  ✓ Identical: $SAME"
echo "  ✗ Different: $DIFFERENT"
echo "  ⚠ Errors:    $ERRORS"
echo "════════════════════════════════════════════════════"

if [[ $DIFFERENT -gt 0 || $ERRORS -gt 0 ]]; then
    exit 1
fi

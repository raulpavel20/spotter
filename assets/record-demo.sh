#!/usr/bin/env bash
# Records assets/demo.gif with vhs (https://github.com/charmbracelet/vhs).
# Needs vhs, ttyd and ffmpeg. Builds a release binary and a throwaway demo
# project, then records the review loop. Run from anywhere:
#   assets/record-demo.sh
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
cargo build --release --manifest-path "$ROOT/Cargo.toml" -q

# --- the demo project: a feature branch an agent has been working on ----
(
mkdir -p "$WORK/shop"; cd "$WORK/shop"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git init -q -b main
git config user.name "Dev"; git config user.email dev@example.com
mkdir -p shop tests
cat > shop/pricing.py <<'PY'
from decimal import Decimal


def line_total(price: Decimal, quantity: int) -> Decimal:
    """Price of one cart line."""
    return price * quantity


def cart_total(lines: list[tuple[Decimal, int]]) -> Decimal:
    total = sum(line_total(p, q) for p, q in lines)
    return round(total, 2)
PY
cat > shop/cart.py <<'PY'
from decimal import Decimal

from .pricing import cart_total


class Cart:
    def __init__(self) -> None:
        self.items: dict[str, tuple[Decimal, int]] = {}

    def add(self, sku: str, price: Decimal, quantity: int = 1) -> None:
        _, current = self.items.get(sku, (price, 0))
        self.items[sku] = (price, current + quantity)

    def total(self) -> Decimal:
        return cart_total(list(self.items.values()))
PY
printf '# shop\n\nA tiny shopping cart.\n' > README.md
git add -A; git commit -qm "Initial cart"
git checkout -qb feature/discounts

cat > shop/pricing.py <<'PY'
from decimal import Decimal


def line_total(price: Decimal, quantity: int) -> Decimal:
    """Price of one cart line."""
    return price * quantity


def apply_discount(total: Decimal, percent: int) -> Decimal:
    """Take `percent` off a total, never going below zero."""
    if not 0 <= percent <= 100:
        raise ValueError(f"discount out of range: {percent}")
    return total * (100 - percent) / 100


def cart_total(lines: list[tuple[Decimal, int]], percent: int = 0) -> Decimal:
    total = sum(line_total(p, q) for p, q in lines)
    return round(apply_discount(total, percent), 2)
PY
git commit -qam "Add percentage discounts"

cat > shop/discounts.py <<'PY'
"""Discount codes and what they are worth."""

CODES: dict[str, int] = {
    "WELCOME10": 10,
    "SPRING25": 25,
}


def percent_for(code: str | None) -> int:
    """The discount for a code; unknown codes are worth nothing."""
    if code is None:
        return 0
    return CODES.get(code.strip().upper(), 0)
PY
cat > shop/cart.py <<'PY'
from decimal import Decimal

from .discounts import percent_for
from .pricing import cart_total


class Cart:
    def __init__(self) -> None:
        self.items: dict[str, tuple[Decimal, int]] = {}
        self.code: str | None = None

    def add(self, sku: str, price: Decimal, quantity: int = 1) -> None:
        _, current = self.items.get(sku, (price, 0))
        self.items[sku] = (price, current + quantity)

    def apply_code(self, code: str) -> None:
        self.code = code

    def total(self) -> Decimal:
        return cart_total(list(self.items.values()), percent_for(self.code))
PY
git add -A; git commit -qm "Support discount codes"

grep -v '    return round(apply_discount(total, percent), 2)' shop/pricing.py > pricing.tmp
echo '    return apply_discount(total, percent).quantize(Decimal("0.01"))' >> pricing.tmp
mv pricing.tmp shop/pricing.py
git commit -qam "Fix rounding of discounted totals"

# Uncommitted work in progress: tests the agent is writing.
cat > tests/test_discounts.py <<'PY'
from decimal import Decimal

from shop.cart import Cart


def test_welcome_code_takes_ten_percent():
    cart = Cart()
    cart.add("mug", Decimal("12.50"), 2)
    cart.apply_code("welcome10")
    assert cart.total() == Decimal("22.50")


def test_unknown_code_is_ignored():
    cart = Cart()
    cart.add("mug", Decimal("12.50"))
    cart.apply_code("NOPE")
    assert cart.total() == Decimal("12.50")
PY
printf '# shop\n\nA tiny shopping cart with discount codes.\n' > README.md
) >/dev/null
mkdir -p "$WORK/home"

# --- the recording -------------------------------------------------------
cat > "$WORK/demo.tape" <<EOF
Output demo.gif
Set Shell "bash"
Set FontSize 15
Set Width 1240
Set Height 720
Set Padding 14
Set Framerate 24
Set Theme "Catppuccin Mocha"
Set TypingSpeed 80ms
Env COLORTERM "truecolor"
Env GIT_CONFIG_GLOBAL "/dev/null"
Env HOME "$WORK/home"
Env XDG_CONFIG_HOME ""

Hide
Type "export PS1='\$ ' PATH=$ROOT/target/release:\$PATH && cd $WORK/shop"
Enter
Type "(sleep 19 && git add -A && git commit -qm 'Add discount tests') > /dev/null 2>&1 & clear"
Enter
Sleep 300ms
Show

Sleep 600ms
Type "spotter"
Sleep 400ms
Enter
Sleep 2.5s
Type "u"
Sleep 1.2s
Enter
Sleep 3s
Space
Sleep 2.5s
Space
Sleep 2s
Space
Sleep 3s
Space
Sleep 3s
Escape
Sleep 3.5s
Type ","
Sleep 2.5s
Escape
Sleep 1.5s
EOF
(cd "$WORK" && vhs demo.tape)
cp "$WORK/demo.gif" "$ROOT/assets/demo.gif"
echo "wrote $ROOT/assets/demo.gif"

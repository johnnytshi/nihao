#!/bin/bash
# Install only authentication components, preserving config, models and enrollment.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname -- "$SCRIPT_DIR")"
if [ "$EUID" -ne 0 ]; then
    echo "Run with sudo: $0 [--kde-face]" >&2
    exit 1
fi

restore_file() {
    local name="$1" target="$2" temporary
    if [ -e "$BACKUP_DIR/$name" ] || [ -L "$BACKUP_DIR/$name" ]; then
        temporary=$(mktemp "$(dirname -- "$target")/.nihao-restore.XXXXXX")
        cp -a -- "$BACKUP_DIR/$name" "$temporary"
        mv -fT -- "$temporary" "$target"
    elif [ -f "$BACKUP_DIR/$name.absent" ]; then
        rm -f -- "$target"
    fi
}

if [ "${1:-}" = "--rollback" ]; then
    [ "$#" -eq 2 ] || exit 1
    BACKUP_DIR=$(realpath -- "$2")
    case "$BACKUP_DIR" in /var/lib/nihao/install-backups/*) ;; *) exit 1 ;; esac
    restore_file module /lib/security/pam_nihao.so
    restore_file helper /usr/lib/nihao/nihao-auth
    restore_file cli /usr/local/bin/nihao
    restore_file kde-face /etc/pam.d/kde-face
    if [ -f "$BACKUP_DIR/kde-user" ] && command -v kwriteconfig6 >/dev/null; then
        preference_user=$(cat "$BACKUP_DIR/kde-user")
        preference_value=$(cat "$BACKUP_DIR/kde-preference")
        if [ "$preference_value" = "__NIHAO_UNSET__" ]; then
            runuser -u "$preference_user" -- kwriteconfig6 --file kscreenlockerrc --group Authenticators --key Face --delete
        else
            runuser -u "$preference_user" -- kwriteconfig6 --file kscreenlockerrc --group Authenticators --key Face "$preference_value"
        fi
    fi
    echo "Restored authentication components from $BACKUP_DIR"
    exit 0
fi
[ "$#" -eq 0 ] || { [ "$#" -eq 1 ] && [ "$1" = "--kde-face" ]; }
for binary in libpam_nihao.so nihao-auth nihao; do
    [ -f "$PROJECT_DIR/target/release/$binary" ] || {
        echo "Build first: cargo build --release --workspace" >&2
        exit 1
    }
done

install -d -o root -g root -m 0755 /usr/lib/nihao /lib/security /usr/local/bin
install -d -o root -g root -m 0700 /var/lib/nihao/install-backups
BACKUP_DIR=$(mktemp -d /var/lib/nihao/install-backups/auth-XXXXXXXX)
backup_file() {
    local target="$1" name="$2"
    if [ -e "$target" ] || [ -L "$target" ]; then
        cp -a -- "$target" "$BACKUP_DIR/$name"
    else
        touch "$BACKUP_DIR/$name.absent"
    fi
}
backup_file /lib/security/pam_nihao.so module
backup_file /usr/lib/nihao/nihao-auth helper
backup_file /usr/local/bin/nihao cli
backup_file /etc/pam.d/kde-face kde-face
if [ "${1:-}" = "--kde-face" ] && [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != root ] \
    && command -v kreadconfig6 >/dev/null && command -v kwriteconfig6 >/dev/null; then
    preference_value=$(runuser -u "$SUDO_USER" -- kreadconfig6 --file kscreenlockerrc --group Authenticators --key Face --default __NIHAO_UNSET__)
    printf '%s\n' "$SUDO_USER" > "$BACKUP_DIR/kde-user"
    printf '%s\n' "$preference_value" > "$BACKUP_DIR/kde-preference"
fi
echo "Backup created: $BACKUP_DIR"
echo "Rollback if needed: sudo $SCRIPT_DIR/install_auth.sh --rollback $BACKUP_DIR"

temporary=""
trap 'if [ -n "$temporary" ]; then rm -f -- "$temporary"; fi' EXIT
install_atomic() {
    local source="$1" target="$2" mode="$3"
    temporary=$(mktemp "$(dirname -- "$target")/.nihao-install.XXXXXX")
    install -o root -g root -m "$mode" -- "$source" "$temporary"
    mv -fT -- "$temporary" "$target"
    temporary=""
}
install_atomic "$PROJECT_DIR/target/release/nihao-auth" /usr/lib/nihao/nihao-auth 4755
install_atomic "$PROJECT_DIR/target/release/libpam_nihao.so" /lib/security/pam_nihao.so 0755
install_atomic "$PROJECT_DIR/target/release/nihao" /usr/local/bin/nihao 0755
if [ "${1:-}" = "--kde-face" ]; then
    install_atomic "$PROJECT_DIR/config/kde-face.pam" /etc/pam.d/kde-face 0644
    if [ -f "$BACKUP_DIR/kde-user" ]; then
        preference_user=$(cat "$BACKUP_DIR/kde-user")
        runuser -u "$preference_user" -- kwriteconfig6 --file kscreenlockerrc --group Authenticators --key Face true
    fi
    echo "KDE face authentication enabled. Select Face on the next lock screen."
fi
echo "Authentication components installed; configuration and enrollment preserved."
echo "Backup: $BACKUP_DIR"
echo "Rollback: sudo $SCRIPT_DIR/install_auth.sh --rollback $BACKUP_DIR"

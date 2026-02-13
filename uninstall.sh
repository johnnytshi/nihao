#!/bin/bash
# NiHao Face Recognition - Uninstall Script

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}NiHao Face Recognition - Uninstall${NC}"
echo

# Check if running as root
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}Error: This script must be run as root (use sudo)${NC}"
    exit 1
fi

echo -e "${YELLOW}This will remove NiHao face authentication from your system.${NC}"
echo
echo "The following will be removed:"
echo "  - PAM module: /lib/security/pam_nihao.so"
echo "  - CLI binary: /usr/local/bin/nihao"
echo "  - PAM configuration (restored from /etc/pam.d/system-auth.backup if available)"
echo
echo "The following will be KEPT (delete manually if needed):"
echo "  - Models: /usr/share/nihao/models/"
echo "  - Config: /etc/nihao/nihao.toml"
echo "  - Stored passwords: /etc/nihao/*.key (if any)"
echo "  - Face data: /var/lib/nihao/faces/"
echo

read -p "Continue with uninstall? (y/N) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo -e "${YELLOW}Uninstall cancelled.${NC}"
    exit 0
fi

echo

# 1. Restore PAM configuration
echo -e "${BLUE}[1/3] Restoring PAM configuration...${NC}"
if [ -f /etc/pam.d/system-auth.backup ]; then
    cp /etc/pam.d/system-auth.backup /etc/pam.d/system-auth
    rm -f /etc/pam.d/system-auth.backup
    echo -e "${GREEN}✓ Restored /etc/pam.d/system-auth from backup${NC}"
elif [ -f /etc/pam.d/system-auth ]; then
    if grep -q "pam_nihao.so" /etc/pam.d/system-auth; then
        sed -i '/pam_nihao.so/d' /etc/pam.d/system-auth
        echo -e "${GREEN}✓ Removed pam_nihao.so from /etc/pam.d/system-auth${NC}"
        echo -e "${YELLOW}  (no backup found, removed nihao lines only)${NC}"
        echo -e "${YELLOW}  If you used service unlock mode, kwallet5 lines may remain — verify manually${NC}"
    else
        echo -e "${YELLOW}⚠ PAM configuration not found in system-auth${NC}"
    fi
fi

# Also check sudo config (in case user only enabled it there)
if [ -f /etc/pam.d/sudo ]; then
    if grep -q "pam_nihao.so" /etc/pam.d/sudo; then
        sed -i '/pam_nihao.so/d' /etc/pam.d/sudo
        echo -e "${GREEN}✓ Removed pam_nihao.so from /etc/pam.d/sudo${NC}"
    fi
fi

# 2. Remove PAM module
echo -e "${BLUE}[2/3] Removing PAM module...${NC}"
if [ -f /lib/security/pam_nihao.so ]; then
    rm -f /lib/security/pam_nihao.so
    echo -e "${GREEN}✓ Removed /lib/security/pam_nihao.so${NC}"
else
    echo -e "${YELLOW}⚠ PAM module not found${NC}"
fi

# 3. Remove CLI binary
echo -e "${BLUE}[3/3] Removing CLI binary...${NC}"
if [ -f /usr/local/bin/nihao ]; then
    rm -f /usr/local/bin/nihao
    echo -e "${GREEN}✓ Removed /usr/local/bin/nihao${NC}"
else
    echo -e "${YELLOW}⚠ CLI binary not found${NC}"
fi

echo
echo -e "${GREEN}✓ NiHao core components uninstalled!${NC}"
echo

# Remove models and config automatically (system files, not user data)
echo -e "${BLUE}Removing models and configuration...${NC}"
if [ -d /usr/share/nihao ]; then
    MODEL_SIZE=$(du -sh /usr/share/nihao 2>/dev/null | cut -f1)
    rm -rf /usr/share/nihao && echo -e "${GREEN}✓ Removed models ($MODEL_SIZE)${NC}"
fi
if [ -d /etc/nihao ]; then
    # Check if there are password files
    if ls /etc/nihao/*.key 1> /dev/null 2>&1; then
        echo -e "${YELLOW}⚠ Found stored password files in /etc/nihao/${NC}"
        read -p "Remove stored passwords? (y/N) " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            rm -f /etc/nihao/*.key && echo -e "${GREEN}✓ Removed stored passwords${NC}"
        fi
    fi
    rm -rf /etc/nihao && echo -e "${GREEN}✓ Removed config${NC}"
fi

echo

# Ask about face data separately (this is user data)
if [ -d /var/lib/nihao ]; then
    FACE_SIZE=$(du -sh /var/lib/nihao 2>/dev/null | cut -f1)
    echo -e "${YELLOW}Face data directory exists: /var/lib/nihao/ ($FACE_SIZE)${NC}"
    echo "This contains your enrolled face embeddings."
    echo
    read -p "Do you want to delete your enrolled face data? (y/N) " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo
        echo -e "${RED}WARNING: This will permanently delete all enrolled faces!${NC}"
        read -p "Are you sure? (type 'yes' to confirm) " -r
        echo
        if [[ $REPLY == "yes" ]]; then
            rm -rf /var/lib/nihao && echo -e "${GREEN}✓ Removed face data${NC}"
        else
            echo -e "${YELLOW}Kept face data at /var/lib/nihao/${NC}"
        fi
    else
        echo -e "${YELLOW}Kept face data at /var/lib/nihao/${NC}"
        echo "To remove later: sudo rm -rf /var/lib/nihao"
    fi
fi

echo
echo "You can now use password authentication normally."
echo "To reinstall, run: sudo ./install.sh"

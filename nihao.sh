#!/bin/bash
# NiHao Face Recognition - Convenience Wrapper Script

# ONNX Runtime is installed system-wide via package manager
# No need to set LD_LIBRARY_PATH

# Enable debug logging by default (can be overridden)
export RUST_LOG="${RUST_LOG:-info}"

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Get current user (fallback to whoami if USER not set)
CURRENT_USER="${USER:-$(whoami)}"

# Function to show usage
usage() {
    echo -e "${BLUE}NiHao Face Recognition${NC}"
    echo ""
    echo "Usage: $0 [command] [options]"
    echo ""
    echo "Commands:"
    echo -e "  ${GREEN}add${NC} [username] [label]    Enroll a new face (defaults to \$USER)"
    echo -e "  ${GREEN}test${NC} [username]           Test face recognition (defaults to \$USER)"
    echo -e "  ${GREEN}remove${NC} [username] [id]    Remove enrolled face(s) (defaults to \$USER)"
    echo -e "  ${GREEN}list${NC} [username]           List enrolled faces (defaults to \$USER)"
    echo -e "  ${GREEN}snapshot${NC} <output.jpg>     Capture camera snapshot"
    echo -e "  ${GREEN}config${NC}                    Show configuration"
    echo ""
    echo "Options:"
    echo -e "  ${YELLOW}-v, --verbose${NC}             Enable debug logging"
    echo -e "  ${YELLOW}-h, --help${NC}                Show this help"
    echo ""
    echo "Examples:"
    echo "  $0 add                      # Enroll yourself"
    echo "  $0 add \"My Face\"            # Enroll yourself with label"
    echo "  $0 test                     # Test yourself"
    echo "  $0 list                     # List your faces"
    echo "  $0 remove face_0            # Remove your face"
    echo "  $0 add johnny \"Johnny's Face\" # Enroll specific user"
    echo "  $0 test johnny              # Test specific user"
    echo "  $0 list johnny              # List faces for specific user"
    echo "  $0 snapshot /tmp/test.jpg   # Capture snapshot"
    echo "  $0 -v test                  # Test with debug logging"
}

# Parse global options
VERBOSE=false
while [[ $# -gt 0 ]]; do
    case $1 in
        -v|--verbose)
            VERBOSE=true
            export RUST_LOG="debug"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            break
            ;;
    esac
done

# Check if command provided
if [ $# -eq 0 ]; then
    usage
    exit 1
fi

COMMAND=$1
shift

# Change to script directory
cd "$SCRIPT_DIR" || exit 1

# Execute command
case $COMMAND in
    add)
        # Check if first arg looks like a label (contains spaces or is quoted) or username
        if [ $# -eq 0 ]; then
            # No args - use $USER, no label
            USERNAME="$CURRENT_USER"
            echo -e "${BLUE}Enrolling face for user: ${GREEN}$USERNAME${NC}"
            cargo run --release --bin nihao -- add "$USERNAME"
        elif [ $# -eq 1 ]; then
            # One arg - could be username or label for $USER
            # If it contains spaces or special chars, treat as label for $USER
            if [[ "$1" == *" "* ]] || [[ "$1" == \"*\" ]]; then
                USERNAME="$CURRENT_USER"
                LABEL="$1"
                echo -e "${BLUE}Enrolling face for user: ${GREEN}$USERNAME${NC} with label: ${GREEN}$LABEL${NC}"
                cargo run --release --bin nihao -- add "$USERNAME" --label "$LABEL"
            else
                # Treat as username
                USERNAME="$1"
                echo -e "${BLUE}Enrolling face for user: ${GREEN}$USERNAME${NC}"
                cargo run --release --bin nihao -- add "$USERNAME"
            fi
        else
            # Multiple args - first is username, rest is label
            USERNAME="$1"
            shift
            LABEL="$*"
            echo -e "${BLUE}Enrolling face for user: ${GREEN}$USERNAME${NC} with label: ${GREEN}$LABEL${NC}"
            cargo run --release --bin nihao -- add "$USERNAME" --label "$LABEL"
        fi
        ;;

    test)
        # Default to $USER if no username provided
        if [ $# -lt 1 ]; then
            USERNAME="$CURRENT_USER"
        else
            USERNAME="$1"
        fi
        echo -e "${BLUE}Testing face recognition for user: ${GREEN}$USERNAME${NC}"
        cargo run --release --bin nihao -- test "$USERNAME"
        ;;

    remove)
        # Parse args: can be "remove [username] [face_id]" or just "remove [face_id]"
        if [ $# -eq 0 ]; then
            # No args - remove all for $USER
            USERNAME="$CURRENT_USER"
            echo -e "${BLUE}Removing all faces for user: ${GREEN}$USERNAME${NC}"
            cargo run --release --bin nihao -- remove "$USERNAME" --all
        elif [ $# -eq 1 ]; then
            # One arg - if it starts with "face_", treat as face_id for $USER
            # Otherwise treat as username
            if [[ "$1" == face_* ]]; then
                USERNAME="$CURRENT_USER"
                FACE_ID="$1"
                echo -e "${BLUE}Removing face ${GREEN}$FACE_ID${NC} for user: ${GREEN}$USERNAME${NC}"
                cargo run --release --bin nihao -- remove "$USERNAME" "$FACE_ID"
            else
                USERNAME="$1"
                echo -e "${BLUE}Removing all faces for user: ${GREEN}$USERNAME${NC}"
                cargo run --release --bin nihao -- remove "$USERNAME" --all
            fi
        else
            # Two args - username and face_id
            USERNAME="$1"
            FACE_ID="$2"
            echo -e "${BLUE}Removing face ${GREEN}$FACE_ID${NC} for user: ${GREEN}$USERNAME${NC}"
            cargo run --release --bin nihao -- remove "$USERNAME" "$FACE_ID"
        fi
        ;;

    list)
        # Default to $USER if no username provided
        if [ $# -gt 0 ]; then
            USERNAME="$1"
        else
            USERNAME="$CURRENT_USER"
        fi
        echo -e "${BLUE}Listing faces for user: ${GREEN}$USERNAME${NC}"
        cargo run --release --bin nihao -- list "$USERNAME"
        ;;

    snapshot)
        if [ $# -lt 1 ]; then
            echo -e "${RED}Error: Output path required${NC}"
            echo "Usage: $0 snapshot <output.jpg>"
            exit 1
        fi
        OUTPUT=$1
        echo -e "${BLUE}Capturing snapshot to: ${GREEN}$OUTPUT${NC}"
        cargo run --release --bin nihao -- snapshot "$OUTPUT"
        ;;

    config)
        echo -e "${BLUE}Showing configuration${NC}"
        cargo run --release --bin nihao -- config --validate
        ;;

    *)
        echo -e "${RED}Error: Unknown command: $COMMAND${NC}"
        usage
        exit 1
        ;;
esac

EXIT_CODE=$?

# Show status message
if [ $EXIT_CODE -eq 0 ]; then
    echo -e "${GREEN}✓ Command completed successfully${NC}"
else
    echo -e "${RED}✗ Command failed with exit code: $EXIT_CODE${NC}"
fi

exit $EXIT_CODE

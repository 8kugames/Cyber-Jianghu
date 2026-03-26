#!/bin/bash

# Version Bump Script (Improved)
# - Fixes pre-commit semantics
# - Updates dependency versions
# - Only triggers on .rs or Cargo.toml changes

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# List of crates to check
CRATES=("agent" "protocol" "server")

# Parse command line arguments
PRE_COMMIT_MODE=false
while [[ $# -gt 0 ]]; do
    case $1 in
        --pre-commit)
            PRE_COMMIT_MODE=true
            shift
            ;;
        --help)
            echo "Usage: $0 [--pre-commit]"
            echo "  --pre-commit  Run in pre-commit mode (check staged changes only)"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Function to check if there are relevant file changes in a crate
has_relevant_changes() {
    local crate=$1
    local crate_path="crates/$crate"

    if [ "$PRE_COMMIT_MODE" = true ]; then
        # Pre-commit mode: check staged changes only
        # Get list of staged files in this crate
        local staged_files
        staged_files=$(git diff --cached --name-only "$crate_path" 2>/dev/null | grep -E '\.(rs|toml|yaml)$' || true)

        if [ -n "$staged_files" ]; then
            return 0  # Has relevant changes
        fi
    else
        # Manual mode: check working directory changes (unstaged + untracked)
        # Check unstaged changes
        local unstaged_files
        unstaged_files=$(git diff --name-only "$crate_path" 2>/dev/null | grep -E '\.(rs|toml|yaml)$' || true)

        if [ -n "$unstaged_files" ]; then
            return 0  # Has relevant changes
        fi

        # Check untracked files
        local untracked_files
        untracked_files=$(git ls-files --others --exclude-standard "$crate_path" 2>/dev/null | grep -E '\.(rs|toml|yaml)$' || true)

        if [ -n "$untracked_files" ]; then
            return 0  # Has relevant changes
        fi
    fi

    return 1  # No relevant changes
}

# Function to get current version from Cargo.toml
get_current_version() {
    local cargo_file=$1
    grep -E '^version[[:space:]]*=' "$cargo_file" | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/'
}

# Function to bump version (increment patch)
bump_version() {
    local version=$1
    local major=$(echo "$version" | cut -d. -f1)
    local minor=$(echo "$version" | cut -d. -f2)
    local patch=$(echo "$version" | cut -d. -f3)

    echo "${major}.${minor}.$((patch + 1))"
}

# Function to update version in Cargo.toml
update_version_in_file() {
    local cargo_file=$1
    local new_version=$2

    # Use sed to update the version
    # On macOS, sed -i requires an extension argument, so we use '' for no extension
    if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' "s/^version[[:space:]]*=.*$/version = \"$new_version\"/" "$cargo_file"
    else
        sed -i "s/^version[[:space:]]*=.*$/version = \"$new_version\"/" "$cargo_file"
    fi
}

# Function to update dependency versions in other crates
update_dependencies() {
    local updated_crate=$1
    local new_version=$2

    # Map crate names to their package names
    local package_name="cyber-jianghu-$updated_crate"

    # Find all crates that depend on this crate
    for crate in "${CRATES[@]}"; do
        if [ "$crate" = "$updated_crate" ]; then
            continue
        fi

        local cargo_file="crates/$crate/Cargo.toml"

        if [ ! -f "$cargo_file" ]; then
            continue
        fi

        # Check if this crate depends on the updated crate (path dependency)
        if ! grep -q "$package_name.*path.*\.\./$updated_crate" "$cargo_file"; then
            continue
        fi

        echo -e "  ${BLUE}→${NC} Updating dependency in $crate"

        # Use perl for more reliable multi-step replacement
        if command -v perl &> /dev/null; then
            # Step 1: Match the entire dependency line and remove ALL version fields
            # This handles both "version, path" and "path, version" orderings
            perl -i -pe '
                if (/\Q'"$package_name"'\E\s*=\s*\{/) {
                    # Remove all version = "x.y.z" occurrences in this line
                    s/,?\s*version\s*=\s*"[0-9]+\.[0-9]+\.[0-9]+"//g;
                    # Clean up leading comma if version was first
                    s/=\s*\{, /= { /g;
                    # Clean up trailing comma before closing brace
                    s/,\s*\}/ }/g;
                }
            ' "$cargo_file"

            # Step 2: Add version field after the opening brace
            perl -i -pe "s|($package_name\s*=\s*\{)|\1 version = \"$new_version\",|" "$cargo_file"

            echo -e "  ${GREEN}✓${NC} Updated $package_name dependency to $new_version in $crate"
        else
            # Fallback to sed if perl is not available
            if [[ "$OSTYPE" == "darwin"* ]]; then
                # Step 1: Remove all version fields
                sed -i '' -E "s/,? version = \"[0-9]+\.[0-9]+\.[0-9]+\"//g" "$cargo_file"
                # Step 2: Add version after opening brace
                sed -i '' -E "s|($package_name = \{)|\{ version = \"$new_version\",|" "$cargo_file"
            else
                # Step 1: Remove all version fields
                sed -i -E "s/,? version = \"[0-9]+\.[0-9]+\.[0-9]+\"//g" "$cargo_file"
                # Step 2: Add version after opening brace
                sed -i -E "s|($package_name = \{)|\{ version = \"$new_version\",|" "$cargo_file"
            fi
            echo -e "  ${GREEN}✓${NC} Updated $package_name dependency to $new_version in $crate"
        fi
    done
}

echo "=========================================="
echo "Version Bump Script"
if [ "$PRE_COMMIT_MODE" = true ]; then
    echo "Mode: Pre-commit (checking staged changes)"
else
    echo "Mode: Manual (checking working directory)"
fi
echo "=========================================="
echo ""

# Check if we're in a git repository
if ! git rev-parse --git-dir > /dev/null 2>&1; then
    echo -e "${RED}Error: Not in a git repository${NC}"
    exit 1
fi

UPDATED_CRATES=()

for crate in "${CRATES[@]}"; do
    crate_path="crates/$crate"
    cargo_file="$crate_path/Cargo.toml"

    # Check if crate exists
    if [ ! -f "$cargo_file" ]; then
        echo -e "${YELLOW}⚠${NC} Crate $crate not found at $cargo_file"
        continue
    fi

    echo -e "${YELLOW}→${NC} Checking $crate..."

    # Check for relevant file changes (.rs or Cargo.toml)
    if has_relevant_changes "$crate"; then
        if [ "$PRE_COMMIT_MODE" = true ]; then
            echo -e "  ${YELLOW}Found staged .rs/Cargo.toml changes in $crate${NC}"
        else
            echo -e "  ${YELLOW}Found .rs/Cargo.toml changes in $crate${NC}"
        fi

        # Get current version
        current_version=$(get_current_version "$cargo_file")

        if [ -z "$current_version" ]; then
            echo -e "  ${RED}✗${NC} Could not parse version from $cargo_file"
            continue
        fi

        # Bump version
        new_version=$(bump_version "$current_version")

        # Update Cargo.toml
        update_version_in_file "$cargo_file" "$new_version"

        echo -e "  ${GREEN}✓${NC} Updated: $current_version → $new_version"

        # Update dependencies in other crates
        update_dependencies "$crate" "$new_version"

        UPDATED_CRATES+=("$crate: $current_version → $new_version")
    else
        echo -e "  ${GREEN}✓${NC} No .rs/Cargo.toml changes"
    fi
done

echo ""
echo "=========================================="
if [ ${#UPDATED_CRATES[@]} -eq 0 ]; then
    echo "No crates with relevant changes found."
    echo "No versions were updated."
else
    echo "Summary of updated versions:"
    for update in "${UPDATED_CRATES[@]}"; do
        echo "  - $update"
    done
    echo ""
    if [ "$PRE_COMMIT_MODE" = true ]; then
        echo "Version files have been added to this commit."
    else
        echo "Remember to commit these changes!"
    fi
fi
echo "=========================================="

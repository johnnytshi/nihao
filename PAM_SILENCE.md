# PAM Module Silence Requirements

## Critical Issue

PAM modules **MUST BE COMPLETELY SILENT** on stdout/stderr. Any output can break:
- Scripts that parse `sudo` output
- Automated systems expecting specific output formats
- Tools that pipe sudo commands
- Security scanners looking for specific patterns

## Why This Matters

```bash
# This should output ONLY "hello"
sudo echo "hello"

# If pam_nihao.so prints "Face detected!" to stdout:
# Output becomes: "Face detected!\nhello"
# This breaks: scripts, tests, automation, parsing
```

## What We Did

### 1. No println!/eprintln! in Code ✓

**Audit Result:**
- `pam-nihao/src/`: ✓ Zero print statements
- `nihao-core/src/`: ✓ Only in test code (doesn't affect PAM)

### 2. Use Syslog for All Logging ✓

```rust
// PAM module uses syslog, not stdout
let _ = syslog::init_unix(syslog::Facility::LOG_AUTH, log_level);

log::info!("NiHao: Face recognized");  // Goes to syslog
log::warn!("NiHao: Authentication failed");  // Goes to syslog
```

**Check logs:**
```bash
sudo journalctl -u sudo -n 20
sudo journalctl -t sudo -n 20
```

### 3. Reduce Log Verbosity in Release Builds ✓

```rust
// Debug builds: INFO level (verbose, for development)
// Release builds: WARN level (errors/warnings only)
#[cfg(debug_assertions)]
let log_level = log::LevelFilter::Info;
#[cfg(not(debug_assertions))]
let log_level = log::LevelFilter::Warn;
```

**Rationale:**
- INFO logs like "Initializing camera" flood syslog
- In production, only log failures/warnings
- Reduces syslog noise on every sudo call

### 4. Safety: Redirect stdout/stderr to /dev/null ✓

```rust
/// Redirect stdout and stderr to /dev/null in RELEASE builds
/// Protects against accidental prints from dependencies
#[cfg(not(debug_assertions))]
fn silence_output() {
    if let Ok(devnull) = OpenOptions::new().write(true).open("/dev/null") {
        let fd = devnull.as_raw_fd();
        unsafe {
            libc::dup2(fd, 1); // Redirect stdout
            libc::dup2(fd, 2); // Redirect stderr
        }
    }
}
```

**Why this is critical:**
- Protects against prints from underlying libraries (ONNX, V4L2, etc.)
- One rogue `println!` in a dependency can break production
- Defense-in-depth: even if something tries to print, it goes to /dev/null

**In debug builds:**
- Output NOT redirected (allows troubleshooting with prints)

### 5. Call silence_output() First Thing

```rust
fn authenticate_impl(pamh: &Pam) -> Result<(), String> {
    // FIRST ACTION: Silence output
    silence_output();

    // Then do authentication...
}
```

## Verification

### Test 1: Ensure sudo is silent

```bash
# Should output ONLY "test"
sudo -k
output=$(sudo echo "test")
echo "Output: '$output'"

# Expected: "test"
# NOT: "Face detected!\ntest" or "Initializing camera...\ntest"
```

### Test 2: Check syslog instead

```bash
# Clear credential cache
sudo -k

# Authenticate
sudo echo "Testing..."

# Check logs (output should be here, not stdout)
sudo journalctl -u sudo -n 5 --no-pager
```

**Expected log entries:**
```
NiHao: Attempting facial authentication for user: johnny
NiHao: Face recognized for user: johnny
NiHao: PAM_AUTHTOK set successfully for service unlock
```

### Test 3: Scripts shouldn't break

```bash
# Script that parses sudo output
result=$(sudo cat /etc/hostname)
if [ "$result" = "$(cat /etc/hostname)" ]; then
    echo "✓ Output matches (no extra junk)"
else
    echo "✗ Output corrupted by PAM module"
fi
```

### Test 4: Verify file descriptors

```bash
# In debug build, stdout/stderr should work
cargo build

# In release build, stdout/stderr redirected to /dev/null
cargo build --release

# Check the actual PAM module behavior
sudo LD_DEBUG=files sudo echo "test" 2>&1 | grep -i "devnull"
```

## Common PAM Output Mistakes

### ❌ BAD: Printing to stdout/stderr
```rust
println!("Face detected!");  // BREAKS SUDO
eprintln!("Error: No face");  // BREAKS SCRIPTS
```

### ✓ GOOD: Logging to syslog
```rust
log::info!("Face detected");    // Goes to journalctl
log::warn!("Error: No face");   // Goes to journalctl
```

### ❌ BAD: INFO logs in production
```rust
// This floods syslog on EVERY sudo call
log::info!("Initializing camera...");
log::info!("Loading models...");
log::info!("Capturing frame 1/10...");
```

### ✓ GOOD: WARN/ERROR only in production
```rust
// Only log problems
log::warn!("Authentication timeout");
log::error!("Failed to load model: {}", e);
```

## PAM Module Communication

PAM modules communicate through:
1. **Return codes** (PamError::SUCCESS, PamError::AUTH_ERR, etc.)
2. **Syslog** (for debugging/auditing)
3. **PAM data items** (like PAM_AUTHTOK)

**NOT through:**
- ❌ stdout/stderr (reserved for calling program)
- ❌ Exit codes (PAM uses return codes)
- ❌ Environment variables (use PAM data items)
- ❌ Files (use PAM data items or syslog)

## Debugging Tips

### When developing (debug build):
- stdout/stderr work normally
- INFO logs enabled
- Can use `println!` temporarily for debugging

### When testing (release build):
- stdout/stderr redirected to /dev/null
- Only WARN/ERROR logs
- Must use `journalctl` to see logs

### To enable verbose logging in release:
Edit `pam-nihao/src/lib.rs` temporarily:
```rust
// Force INFO level even in release
let log_level = log::LevelFilter::Info;
```

Then rebuild:
```bash
cargo build --release
sudo cp target/release/libpam_nihao.so /lib/security/pam_nihao.so
```

**Remember to revert before production!**

## Real-World Impact

### Scenario 1: Automated Deployment Script
```bash
#!/bin/bash
# Deploy script that uses sudo
result=$(sudo deploy.sh)
if [ $? -eq 0 ]; then
    echo "Deployed: $result"
fi
```

**Without silence:**
- Output: "Initializing camera...\nDeployed successfully"
- Script might fail parsing or display garbage

**With silence:**
- Output: "Deployed successfully"
- Script works correctly

### Scenario 2: Ansible/Puppet/Chef
Configuration management tools use sudo and expect specific output formats. Random prints break their parsers.

### Scenario 3: CI/CD Pipelines
```yaml
- run: sudo systemctl restart myservice
  register: result
  failed_when: result.stdout != ""
```

Any stdout causes false failures.

## Summary

✅ **Implemented protections:**
1. No print statements in code
2. All logging via syslog
3. Reduced log verbosity in release (WARN only)
4. Stdout/stderr redirected to /dev/null in release
5. silence_output() called first in authenticate

✅ **Result:**
- PAM module is completely silent on stdout/stderr
- All diagnostics go to syslog (journalctl)
- Scripts/automation/tools work correctly
- Debug builds still allow troubleshooting

## References

- [PAM Module Writers Guide](http://www.linux-pam.org/Linux-PAM-html/mwg-expected-of-module.html)
- [Why PAM modules must be silent](https://stackoverflow.com/questions/27616923/why-do-pam-modules-need-to-be-silent)
- [PAM Best Practices](https://github.com/linux-pam/linux-pam/blob/master/doc/mwg/pam_module_writers_guide.pdf)

Import("env")

import os
import subprocess


def git_output(*args):
    try:
        return subprocess.check_output(
            ["git", *args], cwd=env.subst("$PROJECT_DIR"), text=True
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


sha = git_output("rev-parse", "--short=12", "HEAD")
dirty = git_output("status", "--porcelain") != ""
build_id = f"{sha}{'-dirty' if dirty else ''}"

env.Append(CPPDEFINES=[("REPEATER_BUILD_ID", env.StringifyMacro(build_id))])

# pioarduino's macOS archive keeps the compiler below a target-named directory while
# PlatformIO Core expects it directly below the package root. Add the real packaged location
# without modifying the user's global PlatformIO installation or relying on a manual symlink.
packages_dir = env.subst("$PROJECT_PACKAGES_DIR")
toolchain_bin = os.path.join(
    packages_dir, "toolchain-riscv32-esp", "riscv32-esp-elf", "bin"
)
if os.path.isdir(toolchain_bin):
    env.PrependENVPath("PATH", toolchain_bin)

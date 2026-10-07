package lib

import (
    "os"
    "os/exec"
)

//nolint:errcheck
func Start(command string) {
    exec.Command("sh", "-c", command)
    os.Exit(1)
}

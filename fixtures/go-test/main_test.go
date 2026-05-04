package main

import "testing"

func TestPasses(t *testing.T) {
    if 2+2 != 4 {
        t.Error("math is broken")
    }
}

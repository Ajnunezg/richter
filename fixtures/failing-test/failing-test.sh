#!/bin/bash
echo "Running failing tests..."
echo "FAIL: test_broken_thing (expected 42, got 0)"
echo "FAIL: test_also_broken (assertion failed)"
echo "Tests: 10 passed, 2 failed, 0 skipped"
exit 1

#!/usr/bin/env python3
import os
import sys

os.execv(os.environ["CQX_EXPERIMENT_ZIG"], [os.environ["CQX_EXPERIMENT_ZIG"], "ar", *sys.argv[1:]])

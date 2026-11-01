#!/usr/bin/env python3
"""Make Spotify Basic Pitch's ONNX export loadable by rten.

The tf2onnx export (basic_pitch/saved_models/icassp_2022/nmp.onnx) names some operators and
their output tensors identically. rten keeps operators and values in one namespace, so it
fails with "Missing input ... for op StatefulPartitionedCall:N". This renames every clashing
value (and its uses) by appending "__v". The maths is unchanged: outputs match ONNX Runtime.

    python3 -m pip install onnx
    python3 tools/fix_basic_pitch_onnx.py nmp.onnx nmp_fixed.onnx
"""
import sys

import onnx


def main(src: str, dst: str) -> None:
    m = onnx.load(src)
    nodes = {n.name for n in m.graph.node}
    renamed = {}
    for n in m.graph.node:
        for k, o in enumerate(n.output):
            if o in nodes:
                renamed[o] = o + "__v"
                n.output[k] = renamed[o]
    for n in m.graph.node:
        for k, i in enumerate(n.input):
            if i in renamed:
                n.input[k] = renamed[i]
    for o in m.graph.output:
        if o.name in renamed:
            o.name = renamed[o.name]
    onnx.save(m, dst)
    print(f"renamed {len(renamed)} values -> {dst}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])

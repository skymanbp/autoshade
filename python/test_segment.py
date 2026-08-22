import contextlib
import io
import json
import os
import tempfile
import unittest

import segment


class GesturePromptTests(unittest.TestCase):
    def write_payload(self, payload):
        tmp = tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", delete=False)
        try:
            json.dump(payload, tmp)
            return tmp.name
        finally:
            tmp.close()

    def test_gp1_parser_preserves_reference_order_and_duplicates(self):
        points = [[0.5, 0.5], [0.1, 0.2], [0.1, 0.2], [1.2, -0.1]]
        path = self.write_payload({"version": "gp1", "points": points})
        try:
            self.assertEqual(
                segment.load_prompt_points(path, (0.5, 0.5)),
                [tuple(point) for point in points],
            )
        finally:
            os.remove(path)

    def test_gp1_parser_names_malformed_and_out_of_range_payloads(self):
        cases = [
            {"version": "gp0", "points": [[0.5, 0.5], [0.2, 0.3]]},
            {"version": "gp1", "points": [[0.5, 0.5]]},
            {
                "version": "gp1",
                "points": [[0.5, 0.5]] * (segment.MAX_GESTURE_PROMPT_POINTS + 1),
            },
            {"version": "gp1", "points": [[0.5, 0.5], [float("nan"), 0.3]]},
            {"version": "gp1", "points": [[0.4, 0.5], [0.2, 0.3]]},
        ]
        for payload in cases:
            with self.subTest(payload_version=payload["version"], count=len(payload["points"])):
                path = self.write_payload(payload)
                try:
                    stderr = io.StringIO()
                    with contextlib.redirect_stderr(stderr), self.assertRaises(SystemExit) as cm:
                        segment.load_prompt_points(path, (0.5, 0.5))
                    self.assertEqual(cm.exception.code, 2)
                    self.assertIn("segment.py: --prompt-file", stderr.getvalue())
                finally:
                    os.remove(path)

    def test_model_values_are_exact_clamped_positive_tensor_shapes(self):
        point_values, label_values = segment.sam_prompt_values(
            [(0.0, 1.0), (0.5, 0.25), (-1.0, 2.0)]
        )
        self.assertEqual(
            point_values,
            [[[[0.0, 1023.0], [511.5, 255.75], [0.0, 1023.0]]]],
        )
        self.assertEqual(label_values, [[[1, 1, 1]]])
        self.assertEqual(len(point_values), 1)
        self.assertEqual(len(point_values[0]), 1)
        self.assertEqual(len(point_values[0][0]), 3)
        self.assertTrue(all(label == 1 for label in label_values[0][0]))

    def test_multi_point_capability_mismatch_is_a_named_refusal(self):
        class OldModel:
            def forward(self, pixel_values):
                return pixel_values

        class MultiPointModel:
            def forward(self, pixel_values, input_points=None, input_labels=None):
                return pixel_values, input_points, input_labels

        segment.require_multi_point_capability(OldModel(), 1)
        segment.require_multi_point_capability(MultiPointModel(), 2)
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr), self.assertRaises(SystemExit) as cm:
            segment.require_multi_point_capability(OldModel(), 2)
        self.assertEqual(cm.exception.code, 2)
        self.assertIn("Sam2Model.forward accepting input_points", stderr.getvalue())
        self.assertIn("upgrade transformers", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()

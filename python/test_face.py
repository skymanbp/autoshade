import contextlib
import io
import json
import os
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest import mock

import face


def ring_edges(ring):
    """A closed ring as MediaPipe publishes it: unordered connections."""
    pairs = [(ring[i], ring[(i + 1) % len(ring)]) for i in range(len(ring))]
    return list(reversed(pairs))


class ChainTests(unittest.TestCase):
    def test_connections_become_one_ordered_ring(self):
        ring = [3, 9, 4, 7, 1]
        got = face.chain(ring_edges(ring))
        self.assertEqual(got[0], 1, "starts at the smallest index")
        self.assertEqual(sorted(got), sorted(ring))
        # Every consecutive pair (wrapping) is an edge of the original ring.
        edges = {frozenset(e) for e in ring_edges(ring)}
        for i in range(len(got)):
            self.assertIn(frozenset((got[i], got[(i + 1) % len(got)])), edges)

    def test_connection_objects_and_pairs_read_alike(self):
        objs = [SimpleNamespace(start=a, end=b) for a, b in [(0, 1), (1, 2), (2, 0)]]
        self.assertEqual(face.edges_of(objs), [(0, 1), (1, 2), (2, 0)])
        self.assertEqual(face.edges_of([[0, 1]]), [(0, 1)])

    def test_anything_but_one_closed_ring_is_refused(self):
        cases = {
            "open chain": [(0, 1), (1, 2), (2, 3)],
            "two rings": [(0, 1), (1, 2), (2, 0), (5, 6), (6, 7), (7, 5)],
            "a branch": [(0, 1), (1, 2), (2, 0), (0, 3)],
            "a loop": [(0, 0), (0, 1), (1, 0)],
            "too small": [(0, 1)],
        }
        for what, edges in cases.items():
            with self.subTest(what):
                with self.assertRaises(ValueError):
                    face.chain(edges)


def mesh(points):
    """A landmark list from normalised (x, y) pairs."""
    return [SimpleNamespace(x=x, y=y) for x, y in points]


# A ten-landmark stand-in for the 478-point mesh: 0-3 the outline (a square),
# 4-6 one eye, 7-9 the other.
STUB = [
    (0.25, 0.25), (0.75, 0.25), (0.75, 0.75), (0.25, 0.75),
    (0.3, 0.4), (0.4, 0.4), (0.35, 0.45),
    (0.6, 0.4), (0.7, 0.4), (0.65, 0.45),
]
RINGS = {"oval": [0, 1, 2, 3], "left_eye": [7, 8, 9], "right_eye": [4, 5, 6]}


class ShapeTests(unittest.TestCase):
    def test_outlines_land_in_pixels_with_a_box_and_the_matching_score(self):
        faces = face.shape_faces(
            [mesh(STUB)], [(0.91, [48.0, 30.0, 152.0, 152.0])], 200, 160, RINGS
        )
        self.assertEqual(len(faces), 1)
        f = faces[0]
        self.assertEqual(f["oval"], [[50.0, 40.0], [150.0, 40.0], [150.0, 120.0], [50.0, 120.0]])
        self.assertEqual(f["box"], [50.0, 40.0, 150.0, 120.0])
        self.assertEqual(f["left_eye"][0], [120.0, 64.0])
        self.assertEqual(f["right_eye"][0], [60.0, 64.0])
        self.assertEqual(f["confidence"], 0.91)

    def test_a_detection_elsewhere_is_not_this_faces_score(self):
        faces = face.shape_faces([mesh(STUB)], [(0.99, [0.0, 0.0, 10.0, 10.0])], 200, 160, RINGS)
        self.assertIsNone(faces[0]["confidence"])

    def test_two_faces_do_not_share_one_detection(self):
        right = [(x + 0.5, y) for x, y in STUB]
        faces = face.shape_faces(
            [mesh(STUB), mesh(right)],
            [(0.8, [50.0, 40.0, 150.0, 120.0]), (0.7, [150.0, 40.0, 250.0, 120.0])],
            200, 160, RINGS,
        )
        self.assertEqual([f["confidence"] for f in faces], [0.8, 0.7])
        lone = face.shape_faces([mesh(STUB), mesh(STUB)], [(0.8, [50.0, 40.0, 150.0, 120.0])], 200, 160, RINGS)
        self.assertEqual([f["confidence"] for f in lone], [0.8, None])

    def test_a_short_mesh_is_refused_rather_than_indexed_past(self):
        with self.assertRaises(ValueError):
            face.shape_faces([mesh(STUB[:5])], [], 200, 160, RINGS)

    def test_no_face_is_an_empty_list_not_an_error(self):
        doc = face.document(face.shape_faces([], [], 200, 160, RINGS), 200, 160)
        self.assertEqual(doc, {"version": 1, "backend": face.BACKEND, "width": 200, "height": 160, "faces": []})
        json.dumps(doc)  # serialisable as written

    def test_iou(self):
        self.assertEqual(face.iou([0, 0, 10, 10], [0, 0, 10, 10]), 1.0)
        self.assertEqual(face.iou([0, 0, 10, 10], [20, 20, 30, 30]), 0.0)
        self.assertAlmostEqual(face.iou([0, 0, 10, 10], [5, 0, 15, 10]), 1 / 3)


class RefusalTests(unittest.TestCase):
    def test_a_missing_mediapipe_is_a_plain_sentence_and_exit_2(self):
        stderr = io.StringIO()
        with mock.patch.dict(sys.modules, {"mediapipe": None}):
            with contextlib.redirect_stderr(stderr), self.assertRaises(SystemExit) as cm:
                face.import_mediapipe()
        self.assertEqual(cm.exception.code, 2)
        self.assertIn("face.py: finding faces needs MediaPipe -> pip install mediapipe", stderr.getvalue())

    def test_the_pin_is_a_version_with_a_digest_and_a_byte_cap(self):
        self.assertEqual(len(face.MODEL["sha256"]), 64)
        self.assertGreater(face.MODEL["bytes"], 0)
        self.assertNotIn("/latest/", face.MODEL["url"])
        self.assertTrue(face.MODEL["url"].endswith("/" + face.MODEL["file"]))

    def test_the_result_is_published_whole(self):
        d = tempfile.mkdtemp()
        out = os.path.join(d, "faces.json")
        face.publish(out, '{"faces":[]}')
        with open(out, encoding="utf-8") as f:
            self.assertEqual(f.read(), '{"faces":[]}')
        self.assertEqual(os.listdir(d), ["faces.json"], "no temporary file is left behind")
        os.remove(out)
        os.rmdir(d)


if __name__ == "__main__":
    unittest.main()

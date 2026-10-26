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


# A face planted in a frame, in face-local units: 0-3 the outline (the face's
# box, corner by corner), 4-6 one eye, 7-9 the other — the RINGS layout.
FACE_UNITS = [
    (0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0),
    (0.1, 0.3), (0.3, 0.3), (0.2, 0.4),
    (0.7, 0.3), (0.9, 0.3), (0.8, 0.4),
]


class FakeLandmarker:
    """Stands in for the two models. It sees a planted face when the face is
    at least `resolve` of the shown region's width — the short-range
    detector's blindness to small faces — and outlines it normalised to that
    region. A face the region cuts comes back clipped to the region, the way
    a mesh of half a face hugs the crop's edge."""

    def __init__(self, faces, resolve=0.1, score=0.9):
        self.faces, self.resolve, self.score, self.calls = faces, resolve, score, []

    def __call__(self, box):
        self.calls.append(list(box))
        rx0, ry0, rx1, ry1 = box
        rw, rh = rx1 - rx0, ry1 - ry0
        meshes, dets = [], []
        for fx0, fy0, fx1, fy1 in self.faces:
            cx0, cy0, cx1, cy1 = max(fx0, rx0), max(fy0, ry0), min(fx1, rx1), min(fy1, ry1)
            if cx1 <= cx0 or cy1 <= cy0 or (fx1 - fx0) < self.resolve * rw:
                continue
            pts = []
            for u, v in FACE_UNITS:
                x = min(max(fx0 + u * (fx1 - fx0), cx0), cx1)
                y = min(max(fy0 + v * (fy1 - fy0), cy0), cy1)
                pts.append(((x - rx0) / rw, (y - ry0) / rh))
            meshes.append(mesh(pts))
            dets.append((self.score, [cx0 - rx0, cy0 - ry0, cx1 - rx0, cy1 - ry0]))
        return meshes, dets


W, H = 2000, 1000


class TileSearchTests(unittest.TestCase):
    def test_tiles_cover_the_frame_with_overlapping_neighbours(self):
        for n in face.TILE_GRIDS:
            with self.subTest(n=n):
                ts = face.tiles(W, H, n)
                self.assertEqual(len(ts), n * n)
                self.assertEqual(min(t[0] for t in ts), 0)
                self.assertEqual(max(t[2] for t in ts), W)
                self.assertEqual(max(t[3] for t in ts), H)
                row = ts[:n]
                tw = row[0][2] - row[0][0]
                for a, b in zip(row, row[1:]):
                    self.assertAlmostEqual((a[2] - b[0]) / tw, face.TILE_OVERLAP, delta=0.01)

    def test_a_first_pass_that_finds_all_it_was_asked_for_visits_no_tile(self):
        big = [(800.0, 200.0, 1200.0, 700.0)]
        fake = FakeLandmarker(big)
        faces = face.search(fake, W, H, 1, RINGS)
        self.assertEqual(fake.calls, [[0, 0, W, H]], "only the whole frame is shown")
        meshes, dets = FakeLandmarker(big)([0, 0, W, H])
        self.assertEqual(faces, face.shape_faces(meshes, dets, W, H, RINGS))

    def test_tiles_that_only_see_the_first_pass_faces_change_nothing(self):
        # One large face, eight asked for: the tiles are searched, see the same
        # face (whole or cut), and the result is the first pass's to the byte.
        big = [(700.0, 150.0, 1300.0, 850.0)]
        fake = FakeLandmarker(big)
        faces = face.search(fake, W, H, 8, RINGS)
        self.assertGreater(len(fake.calls), 1, "the tiles were searched")
        meshes, dets = FakeLandmarker(big)([0, 0, W, H])
        first = face.shape_faces(meshes, dets, W, H, RINGS)
        self.assertEqual(json.dumps(face.document(faces, W, H)), json.dumps(face.document(first, W, H)))

    def test_a_small_face_in_one_tile_comes_back_in_whole_frame_pixels(self):
        # 100 px wide: under what the whole frame (200 px) and a 2x2 tile
        # (114 px) resolve, over what a 3x3 tile (80 px) does.
        small = (150.0, 120.0, 250.0, 220.0)
        fake = FakeLandmarker([small])
        faces = face.search(fake, W, H, 8, RINGS)
        self.assertEqual(len(faces), 1)
        f = faces[0]
        x0, y0, x1, y1 = small
        self.assertEqual(f["oval"], [[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
        self.assertEqual(f["box"], [x0, y0, x1, y1])
        self.assertEqual(f["right_eye"][0], [x0 + 10.0, y0 + 30.0])
        self.assertEqual(f["left_eye"][0], [x0 + 70.0, y0 + 30.0])
        self.assertEqual(f["confidence"], 0.9, "the tile's detection is mapped back too")
        self.assertIn(face.tiles(W, H, 3)[0], fake.calls)

    def test_a_face_only_a_4x4_tile_resolves_is_found(self):
        # 70 px wide: under what a 3x3 tile (80 px) resolves, over what a 4x4
        # tile (2000 / 3.25 * 0.1 = 62 px) does.
        tiny = (100.0, 50.0, 170.0, 120.0)
        fake = FakeLandmarker([tiny])
        faces = face.search(fake, W, H, 8, RINGS)
        self.assertEqual([f["box"] for f in faces], [list(tiny)])
        self.assertIn(face.tiles(W, H, 4)[0], fake.calls)
        self.assertEqual(len(fake.calls), 1 + 4 + 9 + 16, "every level is searched once, no more")

    def test_a_face_two_tiles_see_whole_is_reported_once(self):
        # Inside the overlap of the 2x2 grid's top two tiles.
        t = face.tiles(W, H, 2)
        mid = (t[0][2] + t[1][0]) / 2
        shared = (mid - 60.0, 100.0, mid + 60.0, 220.0)
        fake = FakeLandmarker([shared])
        faces = face.search(fake, W, H, 8, RINGS)
        self.assertTrue(any(c == t[0] for c in fake.calls) and any(c == t[1] for c in fake.calls))
        self.assertEqual(len(faces), 1)
        self.assertEqual(faces[0]["box"], list(shared))

    def test_a_face_a_tile_cuts_is_not_added_as_a_part(self):
        # 300 px wide with a 0.2 limit: too small for the whole frame (400 px),
        # wider than the 2x2 grid's 286 px overlap, so both 2x2 tiles that see
        # it see it cut; the 3x3 grid's middle column holds it whole.
        cut = (850.0, 320.0, 1150.0, 500.0)
        fake = FakeLandmarker([cut], resolve=0.2)
        faces = face.search(fake, W, H, 8, RINGS)
        self.assertEqual([f["box"] for f in faces], [list(cut)], "the whole face once, never a part of it")

    def test_max_faces_holds_across_the_tiles(self):
        crowd = [(100.0 + 300.0 * i, 400.0, 190.0 + 300.0 * i, 490.0) for i in range(6)]
        fake = FakeLandmarker(crowd)
        faces = face.search(fake, W, H, 3, RINGS)
        self.assertEqual(len(faces), 3)
        self.assertLessEqual(len(fake.calls), 1 + 4 + 9 + 16)
        everything = face.search(FakeLandmarker(crowd), W, H, 8, RINGS)
        self.assertEqual(len(everything), 6)

    def test_same_face_reads_containment_as_well_as_overlap(self):
        self.assertTrue(face.same_face([0, 0, 100, 100], [10, 10, 40, 40]))
        self.assertTrue(face.same_face([0, 0, 100, 100], [5, 0, 105, 100]))
        self.assertFalse(face.same_face([0, 0, 100, 100], [90, 90, 130, 130]))


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

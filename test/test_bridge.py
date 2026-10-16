"""Camera-free Python boundary checks; requires the component's NumPy/OpenCV."""
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "script"))
import genicam_bridge as bridge
import multi_camera_capture as multi


class BridgeTests(unittest.TestCase):
    def test_real_camera_configuration_never_silently_selects_mock(self):
        self.assertTrue(bridge._is_mock("mock", "demo"))
        with patch.object(bridge, "HAS_HARVESTERS", False):
            with self.assertRaises(RuntimeError):
                bridge._is_mock("/missing/vendor.cti", "real-serial")
        with patch.object(bridge, "HAS_HARVESTERS", True):
            with self.assertRaises(FileNotFoundError):
                bridge._is_mock("/missing/vendor.cti", "real-serial")

    def test_transforming_frame_preserves_its_capture_age(self):
        frame = bridge._fetch_single_capture("mock", "MOCK_CAMERA")
        frame["captured_at"] = 1.0
        resized = bridge._resize_frame_entry(frame, 32, 24)
        encoded = bridge._reencode_frame_entry(resized, "png")
        self.assertEqual(encoded["captured_at"], 1.0)
        self.assertTrue(bridge._is_frame_entry_stale(encoded, 250))

    def test_multi_camera_worker_has_a_deadline(self):
        result = multi._run_worker_process({"serial_number": "MOCK_CAMERA", "cti_path": "mock"},
            {"processTimeoutMs": 1})
        self.assertFalse(result["success"])
        self.assertIn("timed out", result["error"])


if __name__ == "__main__":
    unittest.main()

from __future__ import annotations

import tempfile
import unittest
import zipfile
from pathlib import Path

from callbook_rs import CallBook


def _write_usa_csv_zip(path: Path, csv_text: str) -> None:
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("usa.csv", csv_text)


class CallBookPythonApiTest(unittest.TestCase):
    def test_lookup_profile_and_catalogs_on_synthetic_db(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            ham0 = root / "ham0"
            photos = ham0 / "photos"
            photos.mkdir(parents=True)
            (ham0 / "hamcall.dat").write_bytes(b"headerrecord")
            (ham0 / "hamcall.idx").write_bytes(b"!!! 0 \r\nK0AB 6 \r\nZZZZZZZZ 11 \r\n")
            (photos / "K0AB.JPG").write_text("jpg", encoding="utf-8")
            (ham0 / "interest").write_text("--- Bands\n0010 * 160 meters\n", encoding="utf-8")
            _write_usa_csv_zip(
                ham0 / "usa.csv.zip",
                (
                    '"Callsign","Class","Name","Address","City","State","ZIP","County","License Issue Date","FCC Transaction Type"\n'
                    '"K0AB","E","Example Operator","1 Test Way","Example City","NJ","00000","Example","20200101","LIAUA"\n'
                ),
            )

            with CallBook.open(root) as db:
                with db.lookup("k0ab") as result:
                    self.assertEqual(result.query, "K0AB")
                    self.assertEqual(result.status, "current")
                    self.assertEqual(result.current.fields["state_or_province"], "NJ")

                with db.profile("k0ab") as profile:
                    self.assertEqual(profile.callsign, "K0AB")
                    self.assertEqual(profile.status, "current")
                    self.assertEqual(profile.current.fields["city"], "Example City")
                    self.assertEqual(profile.assets[0].kind, "photo")

                us = db.current_us_lookup("k0ab")
                self.assertIsNotNone(us)
                self.assertEqual(us.fields["state"], "NJ")

                definition = db.interest_definition("0010")
                self.assertIsNotNone(definition)
                self.assertEqual(definition.label, "160 meters")


if __name__ == "__main__":
    unittest.main()

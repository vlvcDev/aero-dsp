import os
import sqlite3
import sys
import tempfile
import unittest
from unittest import mock
from pathlib import Path

CATALOG_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CATALOG_ROOT))

from pedal_catalog.index import list_models, measure_captures_async, scan_models, upsert_model, write_capture_metadata  # noqa: E402


FAKE_CLI = """#!/usr/bin/env python3
import json, sys
for path in sys.argv[1:]:
    print(json.dumps(dict(gain_db=12.5, gear_type='amp', includes_cab=True, path=path)))
"""


class CaptureInfoTests(unittest.TestCase):
    def test_measurements_are_cached_by_sha256_and_listed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cli = root / 'fake-cli'
            cli.write_text(FAKE_CLI)
            cli.chmod(0o755)
            models = root / 'models'
            models.mkdir()
            (models / 'a.nam').write_bytes(b'same')
            (models / 'b.nam').write_bytes(b'same')
            db = root / 'catalog.db'
            scan_models(models, db)
            self.assertIsNone(list_models(db)[0].gain_db)
            with mock.patch.dict(os.environ, {'PEDAL_CAPTURE_INFO_BIN': str(cli)}):
                measure_captures_async(db).join()
            with sqlite3.connect(db) as connection:
                self.assertEqual(connection.execute('SELECT COUNT(*) FROM capture_info').fetchone(), (1,))
            self.assertEqual([(m.gain_db, m.includes_cab) for m in list_models(db)], [(12.5, True)] * 2)

    def test_missing_cli_leaves_fields_null(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'a.nam').write_bytes(b'model')
            db = root / 'catalog.db'
            scan_models(root, db)
            with mock.patch('pedal_catalog.index.capture_info_bin', return_value=None):
                self.assertIsNone(measure_captures_async(db))
            self.assertEqual([(m.gain_db, m.includes_cab) for m in list_models(db)], [(None, None)])


class CatalogIndexTests(unittest.TestCase):
    def test_missing_files_are_hidden_and_gear_survives_metadata_update(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            model, db = root / 'capture.nam', root / 'catalog.db'
            model.write_bytes(b'model')
            write_capture_metadata(model, '1', '2', 'Gain 5', 'Drive', 'pedal', [' Distortion ', 'distortion', '', 42])
            upsert_model(model, db)
            write_capture_metadata(model, '1', '2', 'Gain 5')
            self.assertEqual(list_models(db)[0].gear, 'pedal')
            self.assertEqual(list_models(db)[0].tags, ('distortion',))
            self.assertEqual(list_models(db)[0].capture_group_name, 'Drive')
            model.unlink()
            self.assertEqual(list_models(db), [])

    def test_capture_group_survives_rescan_rebuild_and_later_download(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            models = root / 'models'
            models.mkdir()
            a, b = models / 'gain-1.nam', models / 'gain-2.nam'
            a.write_bytes(b'first')
            write_capture_metadata(a, '7', '1', 'Gain 1', 'Amp head')
            db = root / 'catalog.db'
            upsert_model(a, db)
            scan_models(models, db)
            b.write_bytes(b'second')
            write_capture_metadata(b, '7', '2', 'Gain 2', 'Amp head')
            scan_models(models, db)
            for database in (db, root / 'rebuilt.db'):
                scan_models(models, database)
                records = list_models(database)
                self.assertEqual({r.capture_group for r in records}, {'tone3000:7'})
                self.assertEqual({r.capture_id for r in records}, {'1','2'})
                self.assertEqual({r.capture_group_name for r in records}, {'Amp head'})
                self.assertEqual([r.name for r in records], ['Gain 1','Gain 2'])
            local = models / 'local.nam'
            local.write_bytes(b'local')
            write_capture_metadata(local, '8', '3', 'Gain 1')
            scan_models(models, db)
            self.assertEqual(len({r.capture_group for r in list_models(db)}), 2)

    def test_scan_indexes_only_nam_files_and_updates_changed_content(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            models = root / "models"
            models.mkdir()
            model = models / "Clean Combo.nam"
            model.write_text("first model", encoding="utf-8")
            (models / "Clean Combo Copy.nam").write_text("first model", encoding="utf-8")
            (models / "notes.txt").write_text("ignore me", encoding="utf-8")
            database = root / "catalog.db"

            first = scan_models(models, database)
            model.write_text("updated model", encoding="utf-8")
            second = scan_models(models, database)

            self.assertEqual(len(first), 2)
            self.assertEqual(len(second), 2)
            first_by_name = {record.name: record for record in first}
            second_by_name = {record.name: record for record in second}
            self.assertNotEqual(
                first_by_name["Clean Combo"].sha256,
                second_by_name["Clean Combo"].sha256,
            )
            self.assertEqual(
                first_by_name["Clean Combo Copy"].sha256,
                second_by_name["Clean Combo Copy"].sha256,
            )
            with sqlite3.connect(database) as connection:
                count = connection.execute("SELECT COUNT(*) FROM models").fetchone()[0]
            self.assertEqual(count, 2)

            (models / "Clean Combo Copy.nam").unlink()
            scan_models(models, database)
            self.assertEqual([item.name for item in list_models(database)], ["Clean Combo"])

    def test_downloaded_model_retains_tone3000_provenance(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            model = root / "download.nam"
            model.write_bytes(b"model")
            database = root / "catalog.db"

            record = upsert_model(model, database, source="tone3000:model:99")

            self.assertEqual(record.source, "tone3000:model:99")
            with sqlite3.connect(database) as connection:
                source = connection.execute(
                    "SELECT source FROM models WHERE path = ?", (str(model.resolve()),)
                ).fetchone()[0]
            self.assertEqual(source, "tone3000:model:99")
            scan_models(root, database)
            self.assertEqual(list_models(database)[0].source, 'tone3000:model:99')


if __name__ == "__main__":
    unittest.main()


class LibraryTests(unittest.TestCase):
    def test_kind_filter_recent_sort_favorite_and_legacy_migration(self):
        from pedal_catalog.index import set_favorite, touch_model
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'models').mkdir()
            (root / 'irs').mkdir()
            (root / 'models' / 'a.nam').write_bytes(b'a')
            (root / 'models' / 'b.nam').write_bytes(b'b')
            (root / 'irs' / 'cab.wav').write_bytes(b'wav')
            db = root / 'catalog.db'
            with sqlite3.connect(db) as connection:  # database from before last_used existed
                connection.execute('CREATE TABLE models (sha256 TEXT NOT NULL, name TEXT NOT NULL, path TEXT PRIMARY KEY,'
                                   ' size_bytes INTEGER NOT NULL, modified_ns INTEGER NOT NULL,'
                                   " source TEXT NOT NULL DEFAULT 'local', favorite INTEGER NOT NULL DEFAULT 0)")
            scan_models(root / 'models', db)
            self.assertEqual([(m.name, m.kind) for m in list_models(db, kind='ir')], [('cab', 'ir')])
            self.assertEqual([m.name for m in list_models(db)], ['a', 'b'])
            self.assertTrue(touch_model(db, str(root / 'models' / 'b.nam')))
            self.assertEqual([m.name for m in list_models(db, sort='recent')], ['b', 'a'])
            self.assertIsNotNone(list_models(db, sort='recent')[0].last_used)
            self.assertTrue(set_favorite(db, str(root / 'models' / 'b.nam'), True))
            self.assertTrue(list_models(db)[0].favorite)
            self.assertFalse(touch_model(db, str(root / 'missing.nam')))

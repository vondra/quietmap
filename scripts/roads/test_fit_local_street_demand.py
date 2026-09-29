"""Protect spatial count exclusion before any values enter local-demand fitting."""
import csv
import io
from pathlib import Path
import tempfile
import unittest
import zipfile
from fit_local_street_demand import training_points, fit, predict


class CountHoldoutTest(unittest.TestCase):
    def test_holdout_traffic_is_not_read_and_latest_manual_training_count_wins(self):
        fields = ['road_category', 'estimation_method', 'year', 'latitude', 'longitude',
                  'all_motor_vehicles', 'count_point_id']
        rows = [dict(zip(fields, r)) for r in [
            ['MCU', 'Counted', 2025, 41.3874, 2.1686, 'must never parse this holdout count', 'hold'],
            ['MCU', 'Counted', 2024, 53.59047, -2.32209, 1041, 'train'],
            ['MCU', 'Counted', 2025, 53.59047, -2.32209, 1200, 'train'],
            ['MCU', 'Counted', 2020, 53.59047, -2.32209, 9999, 'excluded-year'],
            ['MCU', 'Estimated', 2025, 53.59047, -2.32209, 9999, 'estimated'],
        ]]
        text = io.StringIO()
        writer = csv.DictWriter(text, fieldnames=fields)
        writer.writeheader()
        writer.writerows(rows)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'dft.zip'
            with zipfile.ZipFile(path, 'w') as archive:
                archive.writestr('dft_traffic_counts_aadf.csv', text.getvalue())
            points = training_points(path)
        self.assertEqual([(r['id'], r['year'], r['aadf']) for r in points], [('train', 2025, 1200)])

    def test_fit_rechecks_holdout_membership_without_trusting_an_input_flag(self):
        with self.assertRaisesRegex(ValueError, 'training squares only'):
            fit([dict(x=259, y=191, holdout=False)])

    def test_predict_cells_match_production_living_streets_share_residential(self):
        parameters = dict(residentialUrban=327, unclassifiedUrban=280, rural=121,
                          throughFactor=1.8, demandScale=0.55, singleTrackFactor=0.58)
        rows = [dict(roadClass=6, builtUp=2, through=False, singleTrack=False, trips=0),
                dict(roadClass=9, builtUp=2, through=False, singleTrack=False, trips=0),
                dict(roadClass=5, builtUp=1, through=True, singleTrack=False, trips=0)]
        self.assertEqual(list(predict(parameters, rows)), [327, 280, 121])


if __name__ == '__main__':
    unittest.main()

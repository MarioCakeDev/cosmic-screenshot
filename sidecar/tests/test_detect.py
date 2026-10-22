import pytest

from nllb_sidecar.detect import (
    DefaultDetector,
    Detector,
    FixedDetector,
    join_lines,
)
from nllb_sidecar.languages import LanguageTable
from pathlib import Path

TABLE = Path(__file__).resolve().parents[1] / "nllb_sidecar" / "data" / "nllb_codes.json"


@pytest.fixture(scope="module")
def table() -> LanguageTable:
    return LanguageTable.load(TABLE)


class FakeModel:
    def __init__(self, label: str = "__label__de", score: float = 0.99):
        self.label = label
        self.score = score
        self.seen: list[str] = []

    def predict(self, text, k=1):
        self.seen.append(text)
        return [self.label], [self.score]


def detector(table, label="__label__de", score=0.99):
    model = FakeModel(label, score)
    return Detector("/nonexistent", table, model=model), model


def test_one_detection_per_capture_on_the_joined_text(table):
    det, model = detector(table)
    detection = det.detect(["Die Lieferung erfolgt", "innerhalb von drei Werktagen"])
    assert detection.iso == "de"
    assert not detection.used_default
    assert len(model.seen) == 1, "detection must run once, not once per line"
    assert "Die Lieferung erfolgt innerhalb von drei Werktagen" == model.seen[0]


def test_join_lines_skips_blanks_and_bounds_the_input():
    assert join_lines(["a", " ", "b"]) == "a b"
    long = ["x" * 3000, "y" * 3000]
    assert len(join_lines(long)) == 4000


def test_a_confident_label_is_trusted(table):
    det, _ = detector(table, "__label__ja", 0.91)
    assert det.detect(["こんにちは"]).iso == "ja"


def test_low_confidence_falls_back_to_the_default(table):
    det, _ = detector(table, "__label__de", 0.11)
    detection = det.detect(["Standort"])
    assert detection.used_default
    assert detection.iso == table.default_source
    assert detection.confidence == pytest.approx(0.11)


def test_an_unknown_label_falls_back_to_the_default(table):
    det, _ = detector(table, "__label__tlh", 0.99)
    detection = det.detect(["nuqneH"])
    assert detection.used_default
    assert detection.iso == table.default_source


def test_empty_capture_falls_back_without_calling_the_model(table):
    det, model = detector(table)
    detection = det.detect(["", "   "])
    assert detection.used_default
    assert detection.confidence == 0.0
    assert model.seen == []


def test_label_aliases_resolve_through_the_table(table):
    det, _ = detector(table, "__label__iw", 0.95)
    assert det.detect(["שלום"]).iso == "he"
    det, _ = detector(table, "__label__zh", 0.95)
    assert det.detect(["这是一个句子"]).iso == "zh"


def test_fixed_detector_is_deterministic():
    detection = FixedDetector("fr", 1.0).detect(["anything"])
    assert detection.iso == "fr"
    assert not detection.used_default


def test_default_detector_reports_the_default(table):
    detection = DefaultDetector(table).detect(["Die Lieferung"])
    assert detection.iso == table.default_source
    assert detection.used_default

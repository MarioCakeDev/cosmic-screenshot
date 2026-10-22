import threading
from types import SimpleNamespace

import pytest

from nllb_sidecar.engine import (
    DEFAULT_BEAM_SIZE,
    EngineError,
    NllbEngine,
    is_hallucinated,
    one_line,
)
from nllb_sidecar.languages import LanguageTable


class FakeTokenizer:
    def __init__(self):
        self.src_lang = None
        self.decoded: list[list[str]] = []
        # Text -> the tokens the real tokenizer would produce. Anything not
        # listed tokenises to a single word token.
        self.pieces: dict[str, list[str]] = {}

    def __call__(self, text, truncation=True, max_length=1024):
        tokens = self.pieces.get(text, [f"▁{text}"])
        return SimpleNamespace(input_ids=[f"id:{token}" for token in tokens])

    def convert_ids_to_tokens(self, ids):
        return [identifier.removeprefix("id:") for identifier in ids]

    def convert_tokens_to_ids(self, tokens):
        return [f"id:{token}" for token in tokens]

    def decode(self, ids, skip_special_tokens=True):
        self.decoded.append(list(ids))
        tokens = [identifier.removeprefix("id:") for identifier in ids]
        if skip_special_tokens:
            tokens = [
                token
                for token in tokens
                if not (token.startswith("<") and token.endswith(">"))
            ]
        return " ".join(f"id:{token}" for token in tokens)


class FakeTranslator:
    """CTranslate2's `translate_batch`, with recorded arguments."""

    def __init__(self, hypotheses: dict[str, list[str]] | None = None):
        self.kwargs: list[dict] = []
        self.batches: list[list[list[str]]] = []
        self.hypotheses = hypotheses or {}

    def translate_batch(self, batch, **kwargs):
        self.kwargs.append(kwargs)
        self.batches.append([list(tokens) for tokens in batch])
        return [
            SimpleNamespace(
                hypotheses=[self.hypotheses.get(tokens[0].removeprefix("id:"), ["▁ok"])]
            )
            for tokens in batch
        ]


def engine_with(tokenizer, translator=None, beam_size=DEFAULT_BEAM_SIZE) -> NllbEngine:
    engine = NllbEngine.__new__(NllbEngine)
    engine._tokenizer = tokenizer
    engine._translator = translator
    engine.beam_size = beam_size
    engine._lock = threading.Lock()
    return engine


def test_the_target_prefix_is_dropped():
    tokenizer = FakeTokenizer()
    engine = engine_with(tokenizer)
    assert engine._decode(["eng_Latn", "▁Hello", "▁world"], "eng_Latn") == "id:▁Hello id:▁world"
    assert "eng_Latn" not in tokenizer.decoded[0]


def test_a_real_first_token_is_not_dropped():
    tokenizer = FakeTokenizer()
    engine = engine_with(tokenizer)
    assert engine._decode(["▁Hello", "▁world"], "eng_Latn") == "id:▁Hello id:▁world"


def test_an_unexpected_language_token_is_still_dropped():
    # Defensive: if CTranslate2 ever echoes a different language token, it must
    # not end up in the rendered overlay.
    tokenizer = FakeTokenizer()
    engine = engine_with(tokenizer)
    assert engine._decode(["deu_Latn", "▁Hallo"], "eng_Latn") == "id:▁Hallo"


def test_a_missing_model_is_one_clear_line(tmp_path):
    with pytest.raises(EngineError) as error:
        NllbEngine(tmp_path, tmp_path, LanguageTable.load())
    message = str(error.value)
    assert message.startswith(f"NLLB model not found at {tmp_path}"), message
    assert "\n" not in message


def test_a_missing_tokenizer_is_one_clear_line(tmp_path):
    (tmp_path / "model.bin").write_bytes(b"not a model")
    (tmp_path / "config.json").write_text("{}")
    with pytest.raises(EngineError) as error:
        NllbEngine(tmp_path, tmp_path / "tokenizer", LanguageTable.load())
    message = str(error.value)
    assert "NLLB tokenizer not found at" in message, message
    assert "\n" not in message


def test_library_noise_is_flattened_to_one_line():
    assert one_line("line one\n  line two\ttab") == "line one line two tab"


def test_beam_four_is_the_default():
    translator = FakeTranslator()
    engine = engine_with(FakeTokenizer(), translator)
    engine.translate(["Hallo"], "eng_Latn", "deu_Latn")
    assert translator.kwargs[0]["beam_size"] == DEFAULT_BEAM_SIZE == 4


def test_the_configured_beam_is_passed_to_the_decoder():
    translator = FakeTranslator()
    engine = engine_with(FakeTokenizer(), translator, beam_size=2)
    engine.translate(["Hallo"], "eng_Latn", "deu_Latn")
    assert translator.kwargs[0]["beam_size"] == 2


def test_an_impossible_beam_is_rejected_at_construction(tmp_path):
    (tmp_path / "model.bin").write_bytes(b"not a model")
    (tmp_path / "config.json").write_text("{}")
    with pytest.raises(EngineError) as error:
        NllbEngine(tmp_path, tmp_path, LanguageTable.load(), beam_size=0)
    assert "between 1 and" in str(error.value)


def test_a_hallucinated_short_label_keeps_the_original_text():
    # The measured beam-1 output for "Abbrechen" on the installed model was a
    # whole sentence about EU countries. Painting that into a button is worse
    # than leaving the German label in place.
    source = ["deu_Latn", "▁Abbrechen", "</s>"]
    hypothesis = [
        "eng_Latn", "▁The", "▁following", "▁is", "▁the", "▁list", "▁of",
        "▁the", "▁countries", "▁of", "▁the", "▁European", "▁Union", ":", "</s>",
    ]
    assert is_hallucinated(source, hypothesis)
    assert engine_translate_with("Abbrechen", source, hypothesis) == "Abbrechen"


def test_a_plausible_short_translation_is_kept():
    source = ["deu_Latn", "▁Abmelden", "</s>"]
    hypothesis = ["eng_Latn", "▁Log", "▁out", "</s>"]
    assert not is_hallucinated(source, hypothesis)
    assert engine_translate_with("Abmelden", source, hypothesis) == "id:▁Log id:▁out"


def test_the_guard_only_applies_to_short_lines():
    # A sentence is allowed to grow: its length follows from its content.
    source = ["deu_Latn", "▁Die", "▁Lieferung", "▁erfolgt", "▁innerhalb", "▁von", "▁drei", "</s>"]
    hypothesis = ["eng_Latn", "▁Delivery", "▁takes", "▁place", "▁within", "▁three", "</s>"]
    assert not is_hallucinated(source, hypothesis)
    long_hypothesis = ["eng_Latn"] + [f"▁word{index}" for index in range(30)]
    assert not is_hallucinated(source, long_hypothesis)


def test_the_guard_allows_three_times_the_source_tokens():
    source = ["deu_Latn", "▁Speichern", "▁unter", "</s>"]  # 2 content tokens
    nine = ["eng_Latn"] + [f"▁word{index}" for index in range(6)]
    ten = ["eng_Latn"] + [f"▁word{index}" for index in range(7)]
    assert not is_hallucinated(source, nine)
    assert is_hallucinated(source, ten)


def engine_translate_with(line, source_tokens, hypothesis) -> str:
    tokenizer = FakeTokenizer()
    tokenizer.pieces[line] = source_tokens
    translator = FakeTranslator({source_tokens[0]: hypothesis})
    engine = engine_with(tokenizer, translator)
    [translated] = engine.translate([line], "eng_Latn", "deu_Latn")
    return translated

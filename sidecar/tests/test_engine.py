import pytest

from nllb_sidecar.engine import EngineError, NllbEngine, one_line
from nllb_sidecar.languages import LanguageTable


class FakeTokenizer:
    def __init__(self):
        self.src_lang = None
        self.decoded: list[list[str]] = []

    def convert_tokens_to_ids(self, tokens):
        return [f"id:{token}" for token in tokens]

    def decode(self, ids, skip_special_tokens=True):
        self.decoded.append(list(ids))
        return " ".join(ids)


def engine_with(tokenizer) -> NllbEngine:
    engine = NllbEngine.__new__(NllbEngine)
    engine._tokenizer = tokenizer
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

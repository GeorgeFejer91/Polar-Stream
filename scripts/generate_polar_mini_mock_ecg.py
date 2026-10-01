"""Generate Polar Mini's bundled, synthetic 60-minute ECG fixture."""

from pathlib import Path

import neurokit2 as nk
import numpy as np


RATE_HZ = 130
DURATION_SECONDS = 60 * 60
SEED = 2709
SCALE_UV = 400
OUTPUT = Path(__file__).resolve().parents[1] / "apps/polar-stream-mini/resources/mock-ecg-60m.i16le"


def main() -> None:
    signal = np.asarray(
        nk.ecg_simulate(
            duration=DURATION_SECONDS,
            sampling_rate=RATE_HZ,
            heart_rate=72,
            noise=0.01,
            method="ecgsyn",
            random_state=SEED,
        ),
        dtype=np.float64,
    )
    assert signal.shape == (RATE_HZ * DURATION_SECONDS,)
    assert np.isfinite(signal).all()
    microvolts = np.rint(signal * SCALE_UV)
    assert np.max(np.abs(microvolts)) < 32_768
    OUTPUT.write_bytes(microvolts.astype("<i2").tobytes())
    print(f"{OUTPUT}: {signal.size} samples, {OUTPUT.stat().st_size} bytes, NeuroKit2 {nk.__version__}")


if __name__ == "__main__":
    main()

"""Record fixture identities and retain only non-redundant inputs."""
import hashlib
import json


def finish_fixtures(output, source, source_hash):
    """Record fixture identities, then discard only unchanged input copies."""
    fixtures = {path: hashlib.sha256(path.read_bytes()).hexdigest()
                for path in output.rglob("*") if path.is_file() and path.suffix.lower() == ".winmd"}
    (output / "fixture-hashes.json").write_text(json.dumps({
        str(path.relative_to(output)): digest for path, digest in fixtures.items()
    }, indent=2), encoding="utf-8")
    for path, digest in fixtures.items():
        if path != source and digest == source_hash:
            path.unlink()

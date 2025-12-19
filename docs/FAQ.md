# FAQ (Engineering)

## Why migrate to Python?

Python lowers the barrier for contributors and accelerates feature iteration while keeping behavior and interfaces predictable.

## Why Platform API v2 only?

It is the official, documented interface. This reduces maintenance risk and aligns with Govee developer policies.

## What about LAN or undocumented IoT paths?

Those are out of scope for the Python v2 implementation. The legacy Rust code remains in `src/`.

## How do I validate discovery mapping?

Use `poetry run govee2mqtt-v2 --dry-run` and review capability output. Add tests in `python/tests/` when expanding mappings.

# Test fixture relocation after the 2026-11-10 restart

- Date: 2026-11-11
- Module: Quality

Planning note. Earlier Quality and Architecture notes cite caches under `/private/tmp` (for example `/private/tmp/vantranscribe-ami-eval/` and an `ami-room-v3` research cache); they are historical and unchanged. Those paths were removed by a machine restart on 2026-11-10.

Current locations: public fixtures and verified model caches in `/Volumes/dev/vantranscribe-eval/test-fixtures` (manifest, README, `env.sh`; helper `crates/vt-session/src/test_fixtures.rs` resolves `$VT_TEST_FIXTURES`, then an existing `/private/tmp` path, then that folder); AMI evaluation data and reproduced models under `/Volumes/dev/vantranscribe-eval`.

Not restored: the four-speaker Chinese fixture (no recorded source), the 507 MB standard zipformer archive (re-downloadable), the hand-built WeSpeaker research packs and `ami-room-v3` run directories. Current behavior: [evaluation](../../features/quality/evaluation/evaluation.md#test-fixtures-and-evaluation-data-locations).

#!/usr/bin/env bash
# 앱을 서버에 올린다. 이 컴퓨터에서만 실행한다(GitHub Actions로는 앱을 배포하지 않는다).
#   deploy/deploy.sh            커밋된 상태를 빌드해 올린다(응답이 없으면 서버가 앞 버전으로 되돌린다)
#   deploy/deploy.sh rollback   바로 앞 버전으로 되돌린다
set -euo pipefail
cd "$(dirname "$0")/.."
. deploy/common.sh

if [ "${1:-}" = rollback ]; then
	"${SSH[@]}" "$DEPLOY_HOST" "sudo /opt/tolkmeet/bin/install-release rollback"
	exit
fi
if [ -n "$(git status --porcelain)" ] && [ "${ALLOW_DIRTY:-}" != 1 ]; then
	echo "커밋하지 않은 변경이 있습니다. 커밋한 뒤 배포하세요(무시하려면 ALLOW_DIRTY=1)."
	exit 1
fi

TARGET=x86_64-unknown-linux-gnu
rustup target add "$TARGET" >/dev/null
cargo zigbuild --release --target "$TARGET.2.39" --manifest-path server/Cargo.toml
pnpm --dir web build

rel="$(date -u +%Y%m%d%H%M%S)-$(git rev-parse --short=12 HEAD)"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
cp "server/target/$TARGET/release/tolkmeet-server" "$stage/"
cp -R web/dist "$stage/web"
COPYFILE_DISABLE=1 tar --no-xattrs -czf - -C "$stage" . | "${SSH[@]}" "$DEPLOY_HOST" "sudo /opt/tolkmeet/bin/install-release $rel"
curl -fsS --max-time 10 https://meet.tolkmeet.com/api/health >/dev/null && echo "https://meet.tolkmeet.com is up ($rel)"

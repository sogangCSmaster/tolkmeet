#!/usr/bin/env bash
# 서버 설정(패키지·서비스·자동 복구·백업)을 이 컴퓨터에서 서버로 보내 적용한다. 여러 번 실행해도 된다.
set -euo pipefail
cd "$(dirname "$0")"
. ./common.sh
# DB 백업 버킷 이름(비밀 아님). 버킷을 만들기 전에는 비어 있어 Litestream을 켜지 않는다.
[ -f backup.env ] && . ./backup.env

# Litestream은 고정한 버전의 .deb를 받고, GitHub이 알려 주는 해시로 확인한다.
LS_VER=v0.5.17
LS_ASSET="litestream-${LS_VER#v}-linux-x86_64.deb"
LS_URL="https://github.com/benbjohnson/litestream/releases/download/${LS_VER}/${LS_ASSET}"
LS_SHA=$(gh api "repos/benbjohnson/litestream/releases/tags/${LS_VER}" --jq ".assets[] | select(.name==\"${LS_ASSET}\") | .digest" | sed 's/^sha256://')
[ -n "$LS_SHA" ] || { echo "could not read the litestream checksum"; exit 1; }

COPYFILE_DISABLE=1 tar --no-xattrs -czf - --exclude=._* . | "${SSH[@]}" "$DEPLOY_HOST" \
	"rm -rf /tmp/tolkmeet-deploy && mkdir -p /tmp/tolkmeet-deploy && tar xzf - -C /tmp/tolkmeet-deploy && \
	 sudo LITESTREAM_URL='$LS_URL' LITESTREAM_SHA256='$LS_SHA' LITESTREAM_BUCKET='${LITESTREAM_BUCKET:-}' /tmp/tolkmeet-deploy/server-setup.sh && \
	 rm -rf /tmp/tolkmeet-deploy"

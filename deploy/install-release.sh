#!/usr/bin/env bash
# 서버 쪽 배포: 표준입력의 tar.gz(tolkmeet-server, web/)를 새 버전 폴더에 풀고 바꾼 뒤,
# 30초 안에 응답이 없으면 바로 앞 버전으로 되돌린다. `rollback`이면 앞 버전으로 되돌리기만 한다.
set -euo pipefail
R=/opt/tolkmeet/releases
CUR=/opt/tolkmeet/current

healthy() {
	for _ in $(seq 1 30); do
		curl -fsS --max-time 2 -o /dev/null http://127.0.0.1:8080/api/health && return 0
		sleep 1
	done
	return 1
}

switch_to() {
	ln -sfn "$1" "$CUR.new"
	mv -T "$CUR.new" "$CUR"
	systemctl restart tolkmeet
}

now=$(readlink -f "$CUR" 2>/dev/null || true)
prev=$(ls -1d "$R"/*/ 2>/dev/null | sed 's#/$##' | grep -vx "$now" | sort | tail -n 1 || true)

if [ "${1:-}" = rollback ]; then
	[ -n "$prev" ] || { echo "no previous release"; exit 1; }
	switch_to "$prev"
	healthy && echo "rolled back to $(basename "$prev")"
	exit 0
fi

rel=${1:?release name}
[[ "$rel" =~ ^[0-9]{14}-[0-9a-f]{7,40}$ ]] || { echo "bad release name"; exit 2; }
dir="$R/$rel"
mkdir -p "$dir"
tar xzf - -C "$dir"
test -x "$dir/tolkmeet-server"
test -f "$dir/web/index.html"
chmod -R a+rX "$dir"

switch_to "$dir"
if ! healthy; then
	echo "new release did not answer /api/health"
	if [ -n "$now" ]; then
		switch_to "$now"
		echo "rolled back to $(basename "$now")"
	fi
	exit 1
fi
# 최근 3개만 남긴다.
ls -1d "$R"/*/ | sed 's#/$##' | sort | head -n -3 | xargs -r rm -rf
echo "deployed $rel"

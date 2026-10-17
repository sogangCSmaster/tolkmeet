import { useEffect, useRef, type FormEvent } from 'react'
import type { MediaState } from '../protocol/MediaState'
import type { RoomInfo } from '../lib/api'
import { useI18n } from '../lib/i18n'
import { useAuth, userName } from '../lib/auth'
import type { LocalMedia } from '../lib/media'
import type { Prefs } from '../lib/prefs'
import { legalUrl } from '../lib/links'
import { meetingPath } from '../lib/meetings'
import { navigate, replacePath } from '../lib/router'
import { Icon } from './Icon'
import { LanguageSelects } from './LanguageSelects'
import { Rich } from './Rich'
import { Header } from './Header'
import { StatusPage } from './StatusPage'
import { VideoTile } from './VideoTile'

type Props = {
  code: string
  local: LocalMedia | null
  /** undefined면 아직 확인 중 */
  room: RoomInfo | undefined
  prefs: Prefs
  media: MediaState
  onPrefsChange: (p: Prefs) => void
  onMediaChange: (m: MediaState) => void
  onJoin: () => void
}

export function Lobby({ code, local, room, prefs, media, onPrefsChange, onMediaChange, onJoin }: Props) {
  const { t, locale } = useI18n()
  const auth = useAuth()
  // 로그인한 주최자는 처음 한 번만 계정의 "회의에서 보이는 이름"을 채워 둔다(지우고 새로 써도 되돌아오지 않게).
  const filled = useRef(false)
  useEffect(() => {
    if (filled.current || auth.status === 'loading') return
    filled.current = true
    if (!prefs.name && auth.status === 'host') onPrefsChange({ ...prefs, name: userName(auth.user) })
  }, [auth, prefs, onPrefsChange])

  // 예약할 때 정한 언어 쌍이 있으면, 손님의 화면 언어가 그중 하나일 때 그 언어를 내 언어로 골라 둔다(한 번만).
  const picked = useRef(false)
  useEffect(() => {
    if (picked.current || auth.status !== 'guest' || !room?.langA || !room.langB) return
    picked.current = true
    const mine = room.langA === locale ? room.langA : room.langB === locale ? room.langB : null
    if (!mine) return
    const other = mine === room.langA ? room.langB : room.langA
    if (prefs.lang !== mine || prefs.partnerLang !== other) onPrefsChange({ ...prefs, lang: mine, partnerLang: other })
  }, [auth.status, room, locale, prefs, onPrefsChange])

  // 자막 화면·대면 대화는 주최자 기기에서만 돈다. 주최자는 그 화면으로 보내고, 손님에게는 들어갈 곳이 없다고 알린다.
  const kind = room?.kind
  const hostOnly = !!kind && kind !== 'meeting'
  useEffect(() => {
    if (!kind || kind === 'meeting') return
    if (auth.status === 'host') replacePath(meetingPath({ code, kind }))
    // 쓸 일이 없으니 잡아 둔 카메라·마이크를 바로 놓는다.
    else if (auth.status === 'guest') local?.stream.getTracks().forEach((tr) => tr.stop())
  }, [kind, auth.status, code, local])
  const hasMic = !!local?.stream.getAudioTracks().length
  const hasCam = !!local?.stream.getVideoTracks().length
  const name = prefs.name.trim()

  function submit(e: FormEvent) {
    e.preventDefault()
    if (name && local && room) onJoin()
  }

  if (hostOnly) {
    if (auth.status !== 'guest') return <div className="shell" />
    return (
      <StatusPage
        icon={kind === 'live' ? 'subtitles' : 'forum'}
        eyebrow={room?.host ? t('lobby.hostMeeting', { host: room.host }) : t('lobby.meeting', { code })}
        title={t('lobby.hostDeviceTitle')}
        body={t('lobby.hostDeviceBody')}
        actions={
          <button className="btn btn-primary" onClick={() => navigate('/')}>
            {t('room.home')}
          </button>
        }
      />
    )
  }

  return (
    <div className="shell">
      <Header />
      <main className="lobby">
        <div className="lobby-preview">
          <VideoTile
            name={name || t('lobby.me')}
            stream={local?.stream ?? null}
            showVideo={hasCam && media.cam}
            micOn={hasMic && media.mic}
            mirror
            muted
            offLabel={local && hasCam ? t('lobby.camOff') : undefined}
          />
          <div className="lobby-controls">
            <button
              className={`ctl ${hasMic && media.mic ? '' : 'off'}`}
              disabled={!hasMic}
              aria-pressed={media.mic}
              aria-label={media.mic ? t('ctl.micOff') : t('ctl.micOn')}
              onClick={() => onMediaChange({ ...media, mic: !media.mic })}
            >
              <Icon name={hasMic && media.mic ? 'mic' : 'mic_off'} />
            </button>
            <button
              className={`ctl ${hasCam && media.cam ? '' : 'off'}`}
              disabled={!hasCam}
              aria-pressed={media.cam}
              aria-label={media.cam ? t('ctl.camOff') : t('ctl.camOn')}
              onClick={() => onMediaChange({ ...media, cam: !media.cam })}
            >
              <Icon name={hasCam && media.cam ? 'videocam' : 'videocam_off'} />
            </button>
          </div>
          {!local && <p className="lobby-note">{t('lobby.preparing')}</p>}
          {local?.error && (
            <p className="form-error" role="alert">
              {t(`media.${local.error.kind}`, { device: t(`media.${local.error.device}`) })}
            </p>
          )}
        </div>

        <form className="lobby-form" onSubmit={submit}>
          <div>
            {/* 주최자 이름(회의에서 보이는 이름)을 알면 함께, 없으면 회의 코드를 보인다. */}
            <p className="status-eyebrow">{room?.host ? t('lobby.hostMeeting', { host: room.host }) : t('lobby.meeting', { code })}</p>
            <h1>{room?.title || t('lobby.title')}</h1>
          </div>
          <label className="field-label">
            {t('lobby.name')}
            <input
              className="input"
              value={prefs.name}
              maxLength={40}
              autoFocus
              onChange={(e) => onPrefsChange({ ...prefs, name: e.target.value })}
              placeholder={t('lobby.namePlaceholder')}
            />
          </label>
          {room?.translation && <LanguageSelects prefs={prefs} onChange={onPrefsChange} />}
          <button className="btn btn-primary btn-lg" disabled={!name || !local || !room}>
            {auth.status === 'host' ? t('lobby.joinNow') : t('lobby.askToJoin')}
          </button>
          {(auth.status !== 'host' || room?.translation) && (
            <p className="lobby-note save-note">
              {auth.status !== 'host' && t('lobby.guestNote')} {room?.translation && t('captions.saveNotice')}
            </p>
          )}
          {/* 손님도 입장 전에 약관·처리방침을 보고, 번역이 켜진 회의면 음성이 Gemini로 간다는 것도 본다. */}
          <p className="lobby-note">
            <Rich
              text={t('lobby.agree')}
              parts={{
                terms: (
                  <a href={legalUrl('terms', locale)} target="_blank" rel="noreferrer">
                    {t('lobby.terms')}
                  </a>
                ),
                privacy: (
                  <a href={legalUrl('privacy', locale)} target="_blank" rel="noreferrer">
                    {t('lobby.privacy')}
                  </a>
                ),
              }}
            />
            {room?.translation && <> {t('lobby.geminiNote')}</>}
          </p>
        </form>
      </main>
    </div>
  )
}

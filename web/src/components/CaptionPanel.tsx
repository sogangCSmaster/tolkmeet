import { useState } from 'react'
import type { Caption } from '../lib/captions'
import { useReviewScroll } from '../lib/reviewScroll'
import { useI18n } from '../lib/i18n'
import type { Prefs } from '../lib/prefs'
import type { SttStatus } from '../lib/stt'
import { Icon } from './Icon'
import { LanguageSelects } from './LanguageSelects'
import { Switch } from './Switch'
import { ToLatest } from './ToLatest'

type Props = {
  prefs: Prefs
  translation: boolean
  isHost: boolean
  captions: Caption[]
  sttStatus: SttStatus | null
  speakerName: (id: string) => string
  onPrefsChange: (p: Prefs) => void
  onTranslationChange: (on: boolean) => void
  onClose: () => void
  /** 확정된 자막이 있을 때만. 지금까지의 원문·번역을 텍스트 파일로 내려받는다. */
  onDownload: (() => void) | null
}

/**
 * 회의실 오른쪽 "대화 기록": 자막 화면을 작게 옮긴 것. 최신 문장이 가장 크고 진하며, 지난 문장일수록 작고 흐려진다.
 * 위로 스크롤해 지난 대화를 보는 동안에는 모든 문장이 같은 크기로 바뀌고, "최신 대화로" 버튼이 뜬다.
 * 언어·번역 켜기는 톱니 버튼을 눌러야 펼쳐진다(평소에는 대화만 보인다).
 */
export function CaptionPanel({
  prefs,
  translation,
  isHost,
  captions,
  sttStatus,
  speakerName,
  onPrefsChange,
  onTranslationChange,
  onClose,
  onDownload,
}: Props) {
  const { t } = useI18n()
  // 처음 입장했는데 번역이 꺼져 있거나 주최자가 아직 고르지 않았으면 설정을 펼쳐 둔다.
  const [settingsOpen, setSettingsOpen] = useState(!translation)
  return (
    <aside className="panel" aria-label={t('history.title')}>
      <div className="panel-head">
        <h2>{t('history.title')}</h2>
        {translation && sttStatus === 'live' && (
          <span className="listening" aria-hidden="true">
            <span className="levels on">
              <i />
              <i />
              <i />
            </span>
            {t('history.listening')}
          </span>
        )}
        <div className="panel-actions">
          {onDownload && (
            <button className="icon-btn" aria-label={t('captions.download')} title={t('captions.download')} onClick={onDownload}>
              <Icon name="download" />
            </button>
          )}
          <button
            className={`icon-btn${settingsOpen ? ' on' : ''}`}
            aria-label={t('history.settings')}
            title={t('history.settings')}
            aria-expanded={settingsOpen}
            aria-controls="caption-settings"
            onClick={() => setSettingsOpen((o) => !o)}
          >
            <Icon name="tune" />
          </button>
          <button className="icon-btn" aria-label={t('captions.close')} title={t('captions.close')} onClick={onClose}>
            <Icon name="close" />
          </button>
        </div>
      </div>
      {settingsOpen && (
        <div className="panel-settings" id="caption-settings">
          {isHost && <Switch checked={translation} onChange={onTranslationChange} label={t('captions.translation')} />}
          {translation && <LanguageSelects prefs={prefs} onChange={onPrefsChange} />}
        </div>
      )}
      {translation ? (
        <>
          {(sttStatus === 'paused' || sttStatus === 'unavailable') && (
            <p className="stt-status" role="status">
              <Icon name="mic_off" />
              {t(sttStatus === 'paused' ? 'captions.sttPaused' : 'captions.sttUnavailable')}
            </p>
          )}
          <Feed captions={captions} speakerName={speakerName} />
          {/* 저장 기능을 모든 참가자에게 알린다(자막은 서버에 남지 않고, 각자 자기 화면의 것만 저장한다). */}
          <p className="panel-note">{t('captions.saveNoticeShort')}</p>
        </>
      ) : (
        <div className="feed">
          <p className="feed-empty">
            {t('captions.off')}
            {isHost && (
              <>
                <br />
                {t('captions.offHost')}
              </>
            )}
          </p>
        </div>
      )}
    </aside>
  )
}

/**
 * 최신순 크기: 마지막 문장이 0단계(가장 큼), 그 위로 1·2·3단계. 사용자가 위로 스크롤하면 따라 내려가지 않고
 * 모든 문장을 같은 크기로 보여 준다. 그사이 새로 확정된 문장 수를 "최신 대화로" 버튼에 센다.
 */
function Feed({ captions, speakerName }: { captions: Caption[]; speakerName: (id: string) => string }) {
  const { t, locale } = useI18n()
  // 취소 기록은 낮은 rev로 되살아나지 않도록 상태에 남기고 화면에서만 숨긴다.
  const visible = captions.filter((c) => !(c.final && c.src === ''))
  const { ref, reviewing, onScroll, toLatest, newCount } = useReviewScroll<HTMLDivElement>(visible.map((c) => String(c.seg)), captions)

  return (
    <div className="feed-wrap">
      <div className={`feed${reviewing ? ' reviewing' : ''}`} role="log" aria-live="polite" ref={ref} onScroll={onScroll}>
        {visible.length === 0 && <p className="feed-empty">{t('captions.empty')}</p>}
        {visible.map((c, i) => {
          const age = Math.min(3, visible.length - 1 - i)
          const translated = c.tgt_lang !== c.src_lang && (c.text != null || !c.final)
          return (
            // 확정 전 줄은 계속 바뀌므로 화면 낭독기에는 확정된 줄만 읽힌다.
            <div key={c.seg} className={`entry age-${age}${c.final ? '' : ' pending'}`} aria-hidden={!c.final}>
              <div className="meta">
                <b>{speakerName(c.speaker)}</b>
                <time>{new Date(c.at).toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit', hour12: false })}</time>
              </div>
              {translated ? (
                <>
                  <p className="src" lang={c.src_lang}>
                    {c.src}
                  </p>
                  <p className="tgt tr-mark" lang={c.text != null ? c.tgt_lang : undefined}>
                    {c.text ?? t('captions.translating')}
                  </p>
                </>
              ) : (
                <p className="tgt" lang={c.src_lang}>
                  {c.src}
                </p>
              )}
            </div>
          )
        })}
      </div>
      {reviewing && <ToLatest newCount={newCount} onClick={toLatest} />}
    </div>
  )
}

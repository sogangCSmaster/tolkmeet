import { useRef } from 'react'
import { useI18n } from '../lib/i18n'
import { useModalDialog } from '../lib/modal'

/** 주최자가 나가기를 누르면 모두의 회의가 끝나므로(되돌릴 수 없다) 한 번 묻는다. */
export function EndMeetingDialog({ onEnd, onClose }: { onEnd: () => void; onClose: () => void }) {
  const { t } = useI18n()
  const ref = useRef<HTMLDialogElement>(null)
  useModalDialog(ref)
  return (
    <dialog ref={ref} className="sheet end-dialog" aria-labelledby="end-title" onClose={onClose}>
      <h2 id="end-title">{t('end.title')}</h2>
      <p className="lead">{t('end.body')}</p>
      <div className="row">
        <button className="btn btn-outline" onClick={() => ref.current?.close()}>
          {t('end.cancel')}
        </button>
        <button className="btn btn-primary" onClick={onEnd}>
          {t('end.confirm')}
        </button>
      </div>
    </dialog>
  )
}

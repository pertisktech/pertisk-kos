import { useEffect, useState } from 'react'
import { Icon } from '../components/Icons'
import ColorLogViewer from '../components/ColorLogViewer'
import { formatDate, formatTime } from '../utils/datetime'
import { jobDurationLabel, jobIsLive, jobKindLabel, jobStatusLabel } from '../utils/jobs'

function useNow(enabled) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!enabled) return undefined
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [enabled])
  return now
}

function JobMeta({ job, now }) {
  const live = jobIsLive(job)
  const date = formatDate(job.created_at)
  const time = formatTime(job.created_at)
  return (
    <div className="job-item-meta">
      <time
        className="job-when"
        dateTime={job.created_at || undefined}
        title={job.created_at || ''}
      >
        <span className="job-date">{date}</span>
        {time ? <span className="job-time">{time}</span> : null}
      </time>
      <span className={`job-duration${live ? ' live' : ''}`}>
        {jobDurationLabel(job, now)}
      </span>
    </div>
  )
}

function JobLogMeta({ job, now }) {
  const time = formatTime(job.created_at)
  return (
    <p className="muted chart-footnote job-log-meta">
      <span>{jobKindLabel(job.kind)}</span>
      <span>{jobStatusLabel(job.status)}</span>
      <time dateTime={job.created_at || undefined}>
        {formatDate(job.created_at)}
        {time ? `, ${time}` : ''}
      </time>
      <span className={jobIsLive(job) ? 'job-duration live' : 'job-duration'}>
        {jobDurationLabel(job, now)}
      </span>
    </p>
  )
}

export default function JobsTab({
  jobs,
  selectedJob,
  selectedJobRow,
  log,
  followLog,
  onSelect,
  onRefresh,
  onFollowChange,
}) {
  const live = jobs.some(jobIsLive)
  const now = useNow(live)

  return (
    <div className="tab-body jobs-tab tab-body-fill">
      <div className="jobs-layout">
        <div className="jobs-list">
          <h3 className="section-label">History</h3>
          {jobs.length === 0 && <p className="muted">No jobs yet.</p>}
          {jobs.map((j) => (
            <button
              key={j.id}
              type="button"
              className={`job-item ${selectedJob === j.id ? 'active' : ''}`}
              onClick={() => onSelect(j.id)}
            >
              <div className="job-item-top">
                <span className="job-kind">{jobKindLabel(j.kind)}</span>
                <span className={`badge ${j.status}`}>{jobStatusLabel(j.status)}</span>
              </div>
              <JobMeta job={j} now={now} />
              {j.error && <span className="error job-err">{j.error}</span>}
            </button>
          ))}
        </div>
        <div className="jobs-log">
          <div className="section-head log-head">
            <h3 className="section-label">
              <Icon name="logs" size={16} /> Log
            </h3>
            <div className="row-actions node-log-actions">
              <button
                type="button"
                className="secondary btn-icon"
                onClick={() => selectedJob && onRefresh(selectedJob)}
                disabled={!selectedJob}
                title="Refresh now"
              >
                Refresh
              </button>
              <button
                type="button"
                className={`secondary btn-icon ${followLog ? 'follow-on' : ''}`}
                onClick={() => onFollowChange(true)}
                title="Follow latest output"
              >
                {followLog ? 'Following' : 'Follow'}
              </button>
            </div>
          </div>
          {selectedJobRow && <JobLogMeta job={selectedJobRow} now={now} />}
          <ColorLogViewer
            text={log}
            empty="(select a job)"
            follow={followLog}
            onFollowChange={onFollowChange}
            className="jobs-log-box"
            aria-label="Job log"
          />
        </div>
      </div>
    </div>
  )
}

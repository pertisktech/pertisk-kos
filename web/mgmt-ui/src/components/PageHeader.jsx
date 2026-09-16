export default function PageHeader({ title, description, actions }) {
  return (
    <div className="page-head">
      <div className="page-head-copy">
        <h1>{title}</h1>
        {description ? <p className="page-desc">{description}</p> : null}
      </div>
      {actions ? <div className="page-head-actions">{actions}</div> : null}
    </div>
  )
}

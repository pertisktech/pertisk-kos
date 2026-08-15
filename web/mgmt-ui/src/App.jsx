import { HashRouter, Navigate, Route, Routes } from 'react-router-dom'
import { ConfirmProvider } from './components/Confirm'
import Layout from './Layout'
import Login from './pages/Login'
import Dashboard from './pages/Dashboard'
import Providers from './pages/Providers'
import Clusters from './pages/ClusterList'
import ClusterDetail from './pages/ClusterDetail'
import NodeDetail from './pages/NodeDetail'
import Machines from './pages/Machines'
import Templates from './pages/Templates'
import Audit from './pages/Audit'
import Settings from './pages/Settings'
import OsPackages from './pages/OsPackages'

export default function App() {
  return (
    <ConfirmProvider>
      <HashRouter>
        <Routes>
          <Route path="/login" element={<Login />} />
          <Route path="/auth/callback" element={<Login />} />
          <Route element={<Layout />}>
            <Route path="/" element={<Dashboard />} />
            <Route path="/clusters" element={<Clusters />} />
            <Route path="/clusters/new" element={<Navigate to="/clusters?new=1" replace />} />
            <Route path="/clusters/:id" element={<ClusterDetail />} />
            <Route path="/clusters/:id/nodes/:nid" element={<NodeDetail />} />
            <Route path="/machines" element={<Machines />} />
            <Route path="/os-packages" element={<OsPackages />} />
            <Route path="/templates" element={<Templates />} />
            <Route path="/providers" element={<Providers />} />
            <Route path="/audit" element={<Audit />} />
            <Route path="/settings" element={<Settings />} />
          </Route>
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </HashRouter>
    </ConfirmProvider>
  )
}

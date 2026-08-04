import { HashRouter, Navigate, Route, Routes } from 'react-router-dom'
import { ConfirmProvider } from './components/Confirm'
import Layout from './Layout'
import Login from './pages/Login'
import Dashboard from './pages/Dashboard'
import Providers from './pages/Providers'
import Clusters from './pages/ClusterList'
import ClusterNew from './pages/ClusterNew'
import ClusterDetail from './pages/ClusterDetail'
import Settings from './pages/Settings'

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
            <Route path="/clusters/new" element={<ClusterNew />} />
            <Route path="/clusters/:id" element={<ClusterDetail />} />
            <Route path="/providers" element={<Providers />} />
            <Route path="/settings" element={<Settings />} />
          </Route>
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </HashRouter>
    </ConfirmProvider>
  )
}

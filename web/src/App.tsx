import { Navigate, Route, Router, type RouteSectionProps } from '@solidjs/router'
import { DashboardsLayout } from './components/DashboardsLayout'
import { Layout } from './components/Layout'
import { AuthProvider, RequireAuth } from './lib/auth'
import { ThemeProvider } from './lib/theme'
import { UnitsProvider } from './lib/units'
import { CarIndex } from './pages/CarIndex'
import { CarSettings } from './pages/CarSettings'
import { ChargeCost } from './pages/ChargeCost'
import { Charges } from './pages/Charges'
import { ChargingStats } from './pages/ChargingStats'
import { DriveStats } from './pages/DriveStats'
import { Efficiency } from './pages/Efficiency'
import { Geofences } from './pages/Geofences'
import { Locations } from './pages/Locations'
import { Mileage } from './pages/Mileage'
import { Overview } from './pages/Overview'
import { ProjectedRange } from './pages/ProjectedRange'
import { Settings } from './pages/Settings'
import { SignIn } from './pages/SignIn'
import { States } from './pages/States'
import { Updates } from './pages/Updates'

export function App() {
  return (
    <ThemeProvider>
      <AuthProvider>
        <UnitsProvider>
        <Router root={Layout}>
          <Route path="/signin" component={SignIn} />
          <Route
            path="/"
            component={() => (
              <RequireAuth>
                <CarIndex />
              </RequireAuth>
            )}
          />
          {/* All routes are implemented; auth-guarded */}
          <Route
            path="/settings"
            component={() => (
              <RequireAuth>
                <Settings />
              </RequireAuth>
            )}
          />
          <Route
            path="/settings/car/:id"
            component={() => (
              <RequireAuth>
                <CarSettings />
              </RequireAuth>
            )}
          />
          <Route
            path="/geofences"
            component={() => (
              <RequireAuth>
                <Geofences />
              </RequireAuth>
            )}
          />
          <Route
            path="/charge/:id/cost"
            component={() => (
              <RequireAuth>
                <ChargeCost />
              </RequireAuth>
            )}
          />
          <Route
            path="/dashboards"
            component={(props: RouteSectionProps) => (
              <RequireAuth>
                <DashboardsLayout>{props.children}</DashboardsLayout>
              </RequireAuth>
            )}
          >
            <Route path="/overview" component={Overview} />
            <Route path="/states" component={States} />
            <Route path="/updates" component={Updates} />
            <Route path="/mileage" component={Mileage} />
            <Route path="/projected-range" component={ProjectedRange} />
            <Route path="/efficiency" component={Efficiency} />
            <Route path="/locations" component={Locations} />
            <Route path="/drive-stats" component={DriveStats} />
            <Route path="/charges" component={Charges} />
            <Route path="/charging-stats" component={ChargingStats} />
            <Route path="/" component={() => <Navigate href="/dashboards/overview" />} />
          </Route>
        </Router>
        </UnitsProvider>
      </AuthProvider>
    </ThemeProvider>
  )
}

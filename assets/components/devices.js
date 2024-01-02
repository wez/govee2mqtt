import { LitElement, html} from "lit";
import { styleMap} from "lit/directives/style-map.js";
import { Task } from '@lit/task';
import { timeAgo } from './timeago.js';

export class DeviceList extends LitElement {
  timer;
  deviceList;

  static properties = {
    id: { type: String },
    label: { type: String },
    value: { type: String },
  };

  constructor() {
    super();
    this.value = "";
  }

  _deviceListTask = new Task(this, {
    task: async ([], {signal}) => {
      const response = await fetch('/api/devices', {signal});
      if (!response.ok) {
        throw new Error(response.status);
      }
      return response.json();
    },
    args: () => []
  });

  render() {
    return this._deviceListTask.render({
      pending: () => {
        if (this.deviceList === undefined) {
          return html`<p>Loading devices...</p>`;
        }
        return this._render_device_list(this.deviceList);
      },
      complete: (devices) => {
        this.deviceList = devices;
        return this._render_device_list(this.deviceList);
      }
    });
  }

  // This causes the element to appear in the normal DOM which gives it
  // access to the imported bootstrap CSS
  // https://stackoverflow.com/a/58462176/149111
  createRenderRoot() {
    return this;
  }

  ensureTimerStarted() {
    if (this.timer === undefined) {
      this.timer = setInterval(() => {
        this._deviceListTask.run();
      }, 5000);
    }
  }

  ensureTimerStopped() {
    clearInterval(this.timer);
    this.timer = undefined;
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    this.ensureTimerStopped();
  }

  connectedCallback() {
    super.connectedCallback();
    this.ensureTimerStarted();
  }

  _set_power_on(e) {
    const device_id = e.target.dataset.id;
    const power = e.target.checked ? 'on' : 'off';
    fetch(`/api/device/${device_id}/power/${power}`);
  }

  _render_item = (item) => {
    const styles =  {
      backgroundColor: item.state ? `rgb(${item.state.color.r}, ${item.state.color.g}, ${item.state.color.b})` : null,
    };

    const updated = item.state ?
      html`${timeAgo(new Date(item.state.updated))}` : html``;

    const source = item.state ?
      html`<span class="badge rounded-pill text-bg-info">${item.state.source}</span>` : html``;

    const power_switch = html`
    <span class="form-switch"><input
      data-id=${item.id}
      class="form-check-input"
      type="checkbox"
      role="switch"
      @click=${this._set_power_on}
      ?checked=${item.state?.on}
    ></span>`;

    return html`
        <tr>
          <td>${item.name}</td>
          <td>${item.room}</td>
          <td>${item.ip}</td>
          <td>${item.sku}</td>
          <td><span class="badge" style=${styleMap(styles)}>&nbsp;</span> ${power_switch}</td>
          <td><tt>${item.id}</tt></td>
          <td style="width: 10em">${updated}</td>
          <td>${source}</td>
        </tr>
        `;
  }

  _render_device_list = (devices) => {
    return html`
        <table class='table'>
          <thead>
            <tr>
              <th scope="col">Name</th>
              <th scope="col">Room</th>
              <th scope="col">IP</th>
              <th scope="col">SKU</th>
              <th scope="col">State</th>
              <th scope="col">ID</th>
              <th scope="col">Last Updated</th>
              <th scope="col">Source</th>
            </tr>
          </thead>
          <tbody>
            ${devices.map(this._render_item)}
          </tbody>
        </table>
          `;
  }
}

customElements.define("gv-device-list", DeviceList);

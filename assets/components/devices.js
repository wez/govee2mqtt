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

  _set_color(e) {
    const device_id = e.target.dataset.id;
    const color = encodeURIComponent(e.target.value);
    console.log(`color will change to ${color}`);
    fetch(`/api/device/${device_id}/color/${color}`);
  }

  _render_item = (item) => {
    const color_value = (item.state?.color.r << 16) | (item.state?.color.g << 8) | (item.state?.color.b);
    const rgb_hex = `#${color_value.toString(16).padStart(6, '0')}`;
    const rgb = item.state ? `rgba(${item.state.color.r}, ${item.state.color.g}, ${item.state.color.b}, ${item.state.brightness})`: null;
    const styles =  {
      backgroundColor: rgb,
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

    const color_picker = html`
      <input
        class="form-control form-control-color"
        data-id=${item.id}
        @change=${this._set_color}
        type="color"
        value=${rgb_hex}>
      `;

    return html`
        <tr>
          <td>${item.name}</td>
          <td>${item.room}</td>
          <td>${item.ip}</td>
          <td>${item.sku}</td>
          <td>${power_switch}</td>
          <td>${color_picker}</td>
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
              <th scope="col">Power</th>
              <th scope="col">Color</th>
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
